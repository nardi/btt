//! Italo stations and the codes that its booking site takes.

use btt_model::GeoCoordinates;
use smol_str::SmolStr;

/// The largest distance in metres between a station of a plan and an Italo station for them to
/// be the same place.
const MATCH_DISTANCE_METERS: f64 = 1_500.0;

/// An Italo station.
#[derive(Debug, Clone, PartialEq)]
pub struct ItaloStation {
    /// Name of the station.
    pub name: SmolStr,
    /// The code the booking site takes, such as `RMT` for Roma Termini.
    pub code: SmolStr,
    /// Where the station is.
    pub coordinates: GeoCoordinates,
}

/// The stations that Italo serves.
#[derive(Debug, Clone)]
pub struct ItaloStations {
    /// Every station.
    pub stations: Vec<ItaloStation>,
}

impl ItaloStations {
    /// Reads the stations from the text of `italo_stations.csv`, which has the columns name,
    /// code, latitude and longitude. Rows that do not parse are left out.
    pub(crate) fn from_csv(text: &str) -> Self {
        let stations = text
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut fields = line.split(',');
                let (name, code) = (fields.next()?, fields.next()?);
                let latitude: f64 = fields.next()?.parse().ok()?;
                let longitude: f64 = fields.next()?.parse().ok()?;
                Some(ItaloStation {
                    name: name.into(),
                    code: code.into(),
                    coordinates: GeoCoordinates::from_degrees(latitude, longitude).ok()?,
                })
            })
            .collect();
        Self { stations }
    }

    /// Returns the Italo station within 1.5 km of a position, the nearest if several are.
    pub fn station_near(&self, coordinates: GeoCoordinates) -> Option<&ItaloStation> {
        self.stations
            .iter()
            .map(|station| {
                (
                    station,
                    station.coordinates.haversine_distance_meters(coordinates),
                )
            })
            .filter(|&(_, distance)| distance <= MATCH_DISTANCE_METERS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(station, _)| station)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FareData;

    #[test]
    fn a_position_finds_the_station_next_to_it() {
        let data = FareData::bundled().unwrap();
        let termini = GeoCoordinates::from_degrees(41.9009, 12.5018).unwrap();
        assert_eq!(
            data.italo_stations.station_near(termini).unwrap().code,
            "RMT"
        );
        let rogoredo = GeoCoordinates::from_degrees(45.4339, 9.2391).unwrap();
        assert_eq!(
            data.italo_stations.station_near(rogoredo).unwrap().code,
            "RG_"
        );
        let amsterdam = GeoCoordinates::from_degrees(52.3791, 4.9003).unwrap();
        assert!(data.italo_stations.station_near(amsterdam).is_none());
    }
}
