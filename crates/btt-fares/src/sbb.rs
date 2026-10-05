//! Swiss stations and the numbers that the SBB booking site takes.

use btt_model::GeoCoordinates;
use smol_str::SmolStr;

/// The largest distance in metres between a station of a plan and a Swiss station for them to
/// be the same place.
const MATCH_DISTANCE_METERS: f64 = 500.0;

/// A Swiss station.
#[derive(Debug, Clone, PartialEq)]
pub struct SbbStation {
    /// The UIC number of the station, such as 8500010 for Basel SBB.
    pub uic: SmolStr,
    /// Name of the station.
    pub name: SmolStr,
    /// Where the station is.
    pub coordinates: GeoCoordinates,
}

/// The stations of Switzerland that have no comma in their name, which are the places with train
/// service and not the bus stops. The names come from the Swiss timetable of
/// opentransportdata.swiss.
#[derive(Debug, Clone)]
pub struct SbbStations {
    /// Every station.
    pub stations: Vec<SbbStation>,
}

impl SbbStations {
    /// Reads the stations from the text of `sbb_stations.csv`, which has the columns uic, name,
    /// latitude and longitude. Rows that do not parse are left out.
    pub(crate) fn from_csv(text: &str) -> Self {
        let stations = text
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut fields = line.split(',');
                let (uic, name) = (fields.next()?, fields.next()?);
                let latitude: f64 = fields.next()?.parse().ok()?;
                let longitude: f64 = fields.next()?.parse().ok()?;
                Some(SbbStation {
                    uic: uic.into(),
                    name: name.into(),
                    coordinates: GeoCoordinates::from_degrees(latitude, longitude).ok()?,
                })
            })
            .collect();
        Self { stations }
    }

    /// Returns the Swiss station within 500 m of a position, the nearest if several are.
    pub fn station_near(&self, coordinates: GeoCoordinates) -> Option<&SbbStation> {
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
    fn a_position_finds_the_swiss_station_next_to_it() {
        let data = FareData::bundled().unwrap();
        let basel = GeoCoordinates::from_degrees(47.5474, 7.5896).unwrap();
        assert_eq!(
            data.sbb_stations.station_near(basel).unwrap().uic,
            "8500010"
        );
        let bern = GeoCoordinates::from_degrees(46.9490, 7.4392).unwrap();
        assert_eq!(data.sbb_stations.station_near(bern).unwrap().uic, "8507000");
        let amsterdam = GeoCoordinates::from_degrees(52.3791, 4.9003).unwrap();
        assert!(data.sbb_stations.station_near(amsterdam).is_none());
    }
}
