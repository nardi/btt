//! Tables of stations with the codes that booking sites take, matched to a ride by position.

use btt_model::GeoCoordinates;
use smol_str::SmolStr;

/// The largest distance in metres between a station of a plan and a station of a table for them
/// to be the same place.
const MATCH_DISTANCE_METERS: f64 = 500.0;

/// A station with the code a booking site takes.
#[derive(Debug, Clone, PartialEq)]
pub struct CodedStation {
    /// The code, such as the UIC number 8500010 of Basel SBB for the SBB site, or the EVA
    /// number 8011160 of Berlin Hbf for the DB site.
    pub code: SmolStr,
    /// Name of the station as the booking site writes it.
    pub name: SmolStr,
    /// Where the station is.
    pub coordinates: GeoCoordinates,
}

/// A table of stations for one booking site.
#[derive(Debug, Clone)]
pub struct StationTable {
    /// Every station.
    pub stations: Vec<CodedStation>,
}

impl StationTable {
    /// Reads a table from CSV text with the columns code, name, latitude and longitude, after a
    /// header line. Rows that do not parse are left out.
    pub(crate) fn from_csv(text: &str) -> Self {
        let stations = text
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut fields = line.split(',');
                let (code, name) = (fields.next()?, fields.next()?);
                let latitude: f64 = fields.next()?.parse().ok()?;
                let longitude: f64 = fields.next()?.parse().ok()?;
                Some(CodedStation {
                    code: code.into(),
                    name: name.into(),
                    coordinates: GeoCoordinates::from_degrees(latitude, longitude).ok()?,
                })
            })
            .collect();
        Self { stations }
    }

    /// Returns the station within 500 m of a position, the nearest if several are.
    ///
    /// Several entries can share a position, such as `Berlin Hbf` and `Berlin Hbf (S-Bahn)`.
    /// Among those, a name without a part in brackets wins, because it names the whole station.
    pub fn station_near(&self, coordinates: GeoCoordinates) -> Option<&CodedStation> {
        self.stations
            .iter()
            .map(|station| {
                (
                    station,
                    station.coordinates.haversine_distance_meters(coordinates),
                )
            })
            .filter(|&(_, distance)| distance <= MATCH_DISTANCE_METERS)
            .min_by(|a, b| {
                a.1.total_cmp(&b.1)
                    .then_with(|| a.0.name.contains('(').cmp(&b.0.name.contains('(')))
            })
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
            data.sbb_stations.station_near(basel).unwrap().code,
            "8500010"
        );
        let bern = GeoCoordinates::from_degrees(46.9490, 7.4392).unwrap();
        assert_eq!(
            data.sbb_stations.station_near(bern).unwrap().code,
            "8507000"
        );
        let amsterdam = GeoCoordinates::from_degrees(52.3791, 4.9003).unwrap();
        assert!(data.sbb_stations.station_near(amsterdam).is_none());
    }

    #[test]
    fn a_position_finds_the_db_station_and_prefers_the_whole_station() {
        let data = FareData::bundled().unwrap();
        let berlin = GeoCoordinates::from_degrees(52.5256, 13.3696).unwrap();
        let station = data.db_stations.station_near(berlin).unwrap();
        assert_eq!(station.code, "8011160");
        assert_eq!(station.name, "Berlin Hbf");
        let bremen = GeoCoordinates::from_degrees(53.0835, 8.8138).unwrap();
        assert_eq!(
            data.db_stations.station_near(bremen).unwrap().code,
            "8000050"
        );
    }
}
