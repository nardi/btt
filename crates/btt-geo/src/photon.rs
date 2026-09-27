//! Client for the Photon geocoder, which searches OpenStreetMap places by name and by position.

use std::num::NonZeroU32;

use btt_model::{GeoCoordinates, PlaceSearchResult};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use serde::Deserialize;
use smallvec::SmallVec;
use smol_str::SmolStr;
use thiserror::Error;

/// Public Photon server run by Komoot.
pub const PUBLIC_PHOTON_BASE_URL: &str = "https://photon.komoot.io";

/// Largest number of results a search returns.
const SEARCH_RESULT_LIMIT: u8 = 8;

/// OpenStreetMap tags a forward search keeps. Photon combines several `osm_tag` filters with
/// "or", so results are settlements or railway stations.
const SEARCH_OSM_TAG_FILTERS: [&str; 6] = [
    "place:city",
    "place:town",
    "place:village",
    "place:suburb",
    "railway:station",
    "railway:halt",
];

/// Search results, which rarely number more than `SEARCH_RESULT_LIMIT`.
pub type PlaceSearchResults = SmallVec<[PlaceSearchResult; 8]>;

/// Errors from a Photon request.
#[derive(Debug, Error)]
pub enum PhotonError {
    /// The request failed or the server answered with an error status.
    #[error("Photon request failed: {0}")]
    Request(#[from] reqwest::Error),
}

/// Sends searches to a Photon server, at most one request per second.
pub struct PhotonClient {
    /// HTTP client with the BTT user agent.
    http_client: reqwest::Client,
    /// Root URL of the Photon server, without a trailing slash.
    base_url: String,
    /// Keeps the request rate within the public server's usage policy.
    rate_limiter: DefaultDirectRateLimiter,
}

impl PhotonClient {
    /// Creates a client for a Photon server.
    ///
    /// `user_agent` should name BTT, its version and the project URL, as the public server
    /// asks.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be built.
    pub fn new(base_url: &str, user_agent: &str) -> Result<Self, PhotonError> {
        let http_client = reqwest::Client::builder().user_agent(user_agent).build()?;
        Ok(Self {
            http_client,
            base_url: base_url.trim_end_matches('/').to_owned(),
            rate_limiter: RateLimiter::direct(Quota::per_second(NonZeroU32::MIN)),
        })
    }

    /// Searches settlements and railway stations by name.
    ///
    /// `language` is a two-letter code such as "en" or "de". Photon falls back to local names
    /// for languages it does not support.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or the answer cannot be parsed.
    pub async fn search(
        &self,
        query: &str,
        language: &str,
    ) -> Result<PlaceSearchResults, PhotonError> {
        let limit = SEARCH_RESULT_LIMIT.to_string();
        let mut parameters: SmallVec<[(&str, &str); 10]> =
            SmallVec::from_slice(&[("q", query), ("limit", limit.as_str()), ("lang", language)]);
        parameters.extend(
            SEARCH_OSM_TAG_FILTERS
                .iter()
                .map(|filter| ("osm_tag", *filter)),
        );
        let url = format!("{}/api/", self.base_url);
        self.fetch(&url, &parameters, ResultNaming::ObjectName)
            .await
    }

    /// Finds the place closest to a position.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or the answer cannot be parsed.
    pub async fn reverse(
        &self,
        coordinates: GeoCoordinates,
        language: &str,
    ) -> Result<Option<PlaceSearchResult>, PhotonError> {
        let latitude = coordinates.latitude.degrees().to_string();
        let longitude = coordinates.longitude.degrees().to_string();
        let parameters = [
            ("lat", latitude.as_str()),
            ("lon", longitude.as_str()),
            ("lang", language),
        ];
        let url = format!("{}/reverse", self.base_url);
        let results = self
            .fetch(&url, &parameters, ResultNaming::SettlementFirst)
            .await?;
        Ok(results.into_iter().next())
    }

    /// Waits for the rate limiter, sends a request and parses the answer.
    async fn fetch(
        &self,
        url: &str,
        parameters: &[(&str, &str)],
        naming: ResultNaming,
    ) -> Result<PlaceSearchResults, PhotonError> {
        self.rate_limiter.until_ready().await;
        let feature_collection: PhotonFeatureCollection = self
            .http_client
            .get(url)
            .query(parameters)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(place_search_results(feature_collection, naming))
    }
}

/// Which Photon property names a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultNaming {
    /// The name of the found object, which a name search matched.
    ObjectName,
    /// The settlement the object lies in. The object closest to a clicked point is often a
    /// street or a building, while the traveller thinks of the town.
    SettlementFirst,
}

/// A Photon answer, which is a `GeoJSON` feature collection.
#[derive(Debug, Deserialize)]
struct PhotonFeatureCollection {
    features: Vec<PhotonFeature>,
}

#[derive(Debug, Deserialize)]
struct PhotonFeature {
    geometry: PhotonPointGeometry,
    properties: PhotonProperties,
}

#[derive(Debug, Deserialize)]
struct PhotonPointGeometry {
    /// Longitude and latitude, in `GeoJSON` order.
    coordinates: [f64; 2],
}

/// The properties BTT reads. Photon sends more, which serde ignores.
#[derive(Debug, Deserialize)]
struct PhotonProperties {
    name: Option<SmolStr>,
    city: Option<SmolStr>,
    state: Option<SmolStr>,
    country: Option<SmolStr>,
}

/// Turns Photon features into search results. Features without a name or with coordinates out
/// of range are skipped.
fn place_search_results(
    feature_collection: PhotonFeatureCollection,
    naming: ResultNaming,
) -> PlaceSearchResults {
    feature_collection
        .features
        .into_iter()
        .filter_map(|feature| {
            let [longitude, latitude] = feature.geometry.coordinates;
            let coordinates = GeoCoordinates::from_degrees(latitude, longitude).ok()?;
            let properties = feature.properties;
            let name = match naming {
                ResultNaming::ObjectName => properties.name?,
                ResultNaming::SettlementFirst => properties.city.clone().or(properties.name)?,
            };
            // The city is only context when it differs from the name, as for a station.
            let city = properties.city.filter(|city| *city != name);
            let context_parts = [city, properties.state, properties.country];
            let context = context_parts
                .iter()
                .flatten()
                .map(SmolStr::as_str)
                .collect::<Vec<_>>();
            Some(PlaceSearchResult {
                name,
                context: context.join(", ").into(),
                coordinates,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIEN_ANSWER: &str = r#"{
        "type": "FeatureCollection",
        "features": [
            {
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [16.3725042, 48.2083537] },
                "properties": {
                    "osm_key": "place", "osm_value": "city", "name": "Wien",
                    "country": "Österreich", "state": "Wien", "countrycode": "AT"
                }
            },
            {
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [16.3761, 48.1852] },
                "properties": {
                    "osm_key": "railway", "osm_value": "station", "name": "Wien Hauptbahnhof",
                    "city": "Wien", "country": "Österreich", "state": "Wien"
                }
            },
            {
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [16.0, 48.0] },
                "properties": { "osm_key": "place", "osm_value": "village" }
            }
        ]
    }"#;

    #[test]
    fn parses_photon_answer_into_search_results() {
        let feature_collection: PhotonFeatureCollection =
            serde_json::from_str(WIEN_ANSWER).unwrap();
        let results = place_search_results(feature_collection, ResultNaming::ObjectName);
        assert_eq!(results.len(), 2, "the feature without a name is skipped");
        assert_eq!(results[0].name, "Wien");
        assert_eq!(results[0].context, "Wien, Österreich");
        assert_eq!(results[1].name, "Wien Hauptbahnhof");
        assert_eq!(results[1].context, "Wien, Wien, Österreich");
        assert_eq!(results[1].coordinates.latitude.microdegrees(), 48_185_200);
    }

    #[test]
    fn reverse_answers_name_the_settlement() {
        let feature_collection: PhotonFeatureCollection =
            serde_json::from_str(WIEN_ANSWER).unwrap();
        let results = place_search_results(feature_collection, ResultNaming::SettlementFirst);
        assert_eq!(
            results[1].name, "Wien",
            "the station lies in the city of Wien"
        );
        assert_eq!(results[1].context, "Wien, Österreich");
    }

    /// Sends one real request to the public server. Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "sends a request to the public Photon server"]
    async fn searches_public_server() {
        let client = PhotonClient::new(PUBLIC_PHOTON_BASE_URL, "BTT tests").unwrap();
        let results = client.search("Salzburg", "en").await.unwrap();
        assert!(
            results
                .iter()
                .any(|result| result.name.contains("Salzburg"))
        );
    }
}
