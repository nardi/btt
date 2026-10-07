//! Prices, the Interrail pass catalogue and the reservation rules.
//!
//! The data lives in `data/fares/` as TOML and is compiled into this crate, so pricing needs no
//! files at run time and no network.

mod country;
mod distance;
mod italo;
mod passes;
mod price_book;
mod reservations;
mod station_table;
mod tariff;

use thiserror::Error;

pub use country::country_of_time_zone;
pub use distance::DistanceModel;
pub use italo::{ItaloStation, ItaloStations};
pub use passes::{PassCatalogue, PassKind, PassPrices, PassType, PriceAgeGroup, Validity};
pub use price_book::{FreeTravel, PriceBook, RideFacts, TicketPriceSource};
pub use reservations::{ReservationMode, ReservationRule, ReservationRules, RuleScope, SeatFee};
pub use station_table::{CodedStation, StationTable};
pub use tariff::{BookingWindow, DistanceTariff, ExchangeRates};

/// The Interrail pass list, as it is compiled in.
const PASSES_TOML: &str = include_str!("../../../data/fares/interrail_passes.toml");
/// The Interrail rules, as they are compiled in.
const RULES_TOML: &str = include_str!("../../../data/fares/interrail_rules.toml");
/// The reservation rules, as they are compiled in.
const RESERVATIONS_TOML: &str = include_str!("../../../data/fares/interrail_reservations.toml");
/// The tariffs and the distance model, as they are compiled in.
const TARIFFS_TOML: &str = include_str!("../../../data/fares/tariffs.toml");
/// The Italo stations, as they are compiled in.
const ITALO_STATIONS_CSV: &str = include_str!("../../../data/fares/italo_stations.csv");
/// The Swiss stations, as they are compiled in.
const SBB_STATIONS_CSV: &str = include_str!("../../../data/fares/sbb_stations.csv");
/// The stations of the DB booking site with their EVA numbers, from the Trainline stations
/// table, as they are compiled in.
const DB_STATIONS_CSV: &str = include_str!("../../../data/fares/db_stations.csv");
/// The stations of the Trenitalia booking site with their location ids, as they are compiled in.
const TRENITALIA_STATIONS_CSV: &str = include_str!("../../../data/fares/trenitalia_stations.csv");
/// The stations of the ÖBB shop with its station numbers, as they are compiled in.
const OBB_STATIONS_CSV: &str = include_str!("../../../data/fares/obb_stations.csv");
/// The stations of the PKP Intercity booking site with its station numbers, as they are compiled
/// in.
const PKP_STATIONS_CSV: &str = include_str!("../../../data/fares/pkp_stations.csv");
/// The foreign stations with the UIC numbers that the SBB site takes, as they are compiled in.
const SBB_ABROAD_STATIONS_CSV: &str = include_str!("../../../data/fares/sbb_abroad_stations.csv");
/// The stations of the NS booking site with their UIC numbers, as they are compiled in.
const NS_STATIONS_CSV: &str = include_str!("../../../data/fares/ns_stations.csv");
/// The Danish stations with the UIC numbers that DSB takes, as they are compiled in.
const DSB_STATIONS_CSV: &str = include_str!("../../../data/fares/dsb_stations.csv");
/// The Swedish stations with the Samtrafiken ids that SJ takes, as they are compiled in.
const SJ_STATIONS_CSV: &str = include_str!("../../../data/fares/sj_stations.csv");
/// The stops of Snälltåget with the codes its site takes, as they are compiled in.
const SNALLTAGET_STATIONS_CSV: &str = include_str!("../../../data/fares/snalltaget_stations.csv");
/// The Hungarian stations with the codes that MÁV takes, as they are compiled in.
const MAV_STATIONS_CSV: &str = include_str!("../../../data/fares/mav_stations.csv");
/// The Spanish stations with the codes that Renfe takes, as they are compiled in.
const RENFE_STATIONS_CSV: &str = include_str!("../../../data/fares/renfe_stations.csv");
/// The stations of Eurostar with their UIC numbers, as they are compiled in.
const EUROSTAR_STATIONS_CSV: &str = include_str!("../../../data/fares/eurostar_stations.csv");
/// The booking windows, as they are compiled in.
const BOOKING_WINDOWS_TOML: &str = include_str!("../../../data/fares/booking_windows.toml");

/// A data file that does not match what the crate expects.
#[derive(Debug, Error)]
pub enum FareDataError {
    /// A TOML file could not be parsed.
    #[error("invalid fare data in {file}: {source}")]
    Invalid {
        /// The file name.
        file: &'static str,
        /// What the parser reported.
        source: toml::de::Error,
    },
    /// A value in a file is out of range or inconsistent.
    #[error("invalid value in {file}: {message}")]
    InvalidValue {
        /// The file name.
        file: &'static str,
        /// What is wrong.
        message: String,
    },
}

/// Everything that prices a ticket, read from the data files.
#[derive(Debug, Clone)]
pub struct FareData {
    /// The Interrail passes and their prices.
    pub passes: PassCatalogue,
    /// Which rides need a seat reservation on a pass, and for how much.
    pub reservations: ReservationRules,
    /// The rough price per kilometre.
    pub distance_model: DistanceModel,
    /// Tariffs that price the domestic rides of one country.
    pub tariffs: Vec<DistanceTariff>,
    /// The stations of Italo with the codes of its booking site.
    pub italo_stations: ItaloStations,
    /// The Swiss stations with their UIC numbers.
    pub sbb_stations: StationTable,
    /// The stations of the DB booking site with their EVA numbers.
    pub db_stations: StationTable,
    /// The stations of the Trenitalia booking site with their location ids.
    pub trenitalia_stations: StationTable,
    /// The stations of the ÖBB shop with its station numbers.
    pub obb_stations: StationTable,
    /// The stations of the PKP Intercity booking site with its station numbers.
    pub pkp_stations: StationTable,
    /// The stations of the NS booking site with their UIC numbers.
    pub ns_stations: StationTable,
    /// The French, Italian and Austrian stations with the UIC numbers that the SBB site takes,
    /// which sells trips from Switzerland to these countries and French trips too.
    pub sbb_abroad_stations: StationTable,
    /// The Danish stations with the UIC numbers that DSB takes.
    pub dsb_stations: StationTable,
    /// The Swedish stations with the Samtrafiken ids that SJ takes.
    pub sj_stations: StationTable,
    /// The stops of Snälltåget with the codes its site takes.
    pub snalltaget_stations: StationTable,
    /// The Hungarian stations with the codes that MÁV takes.
    pub mav_stations: StationTable,
    /// The Spanish stations with the codes that Renfe takes.
    pub renfe_stations: StationTable,
    /// The stations of Eurostar with their UIC numbers.
    pub eurostar_stations: StationTable,
    /// Fixed rates to euro, for when the ECB reference rates cannot be fetched.
    pub exchange_rates: ExchangeRates,
    /// How far ahead tickets can be bought, by country.
    pub booking_windows: Vec<BookingWindow>,
    /// How many outbound and inbound journeys a pass allows in the country of residence.
    pub residence_journeys_each_way: u8,
}

/// The parts of `interrail_rules.toml` that the crate reads.
#[derive(serde::Deserialize)]
struct RulesFile {
    /// The rule for the country of residence.
    residence: ResidenceRule,
}

/// Journeys a pass allows in the country of residence.
#[derive(serde::Deserialize)]
struct ResidenceRule {
    /// Journeys out of the country, and the same number into it.
    outbound_journeys: u8,
}

impl FareData {
    /// Reads the compiled-in data files.
    ///
    /// # Errors
    ///
    /// Returns an error when a file does not parse or holds an impossible value, which the tests
    /// of this crate rule out for the files in the repository.
    pub fn bundled() -> Result<Self, FareDataError> {
        let rules: RulesFile =
            toml::from_str(RULES_TOML).map_err(|source| FareDataError::Invalid {
                file: "interrail_rules.toml",
                source,
            })?;
        Ok(Self {
            passes: PassCatalogue::from_toml(PASSES_TOML)?,
            reservations: ReservationRules::from_toml(RESERVATIONS_TOML, RULES_TOML)?,
            distance_model: DistanceModel::from_toml(TARIFFS_TOML)?,
            tariffs: tariff::tariffs_from_toml(TARIFFS_TOML)?,
            italo_stations: ItaloStations::from_csv(ITALO_STATIONS_CSV),
            sbb_stations: StationTable::from_csv(SBB_STATIONS_CSV),
            db_stations: StationTable::from_csv(DB_STATIONS_CSV),
            trenitalia_stations: StationTable::from_csv(TRENITALIA_STATIONS_CSV),
            obb_stations: StationTable::from_csv(OBB_STATIONS_CSV),
            pkp_stations: StationTable::from_csv(PKP_STATIONS_CSV),
            ns_stations: StationTable::from_csv(NS_STATIONS_CSV),
            sbb_abroad_stations: StationTable::from_csv(SBB_ABROAD_STATIONS_CSV),
            dsb_stations: StationTable::from_csv(DSB_STATIONS_CSV),
            sj_stations: StationTable::from_csv(SJ_STATIONS_CSV),
            snalltaget_stations: StationTable::from_csv(SNALLTAGET_STATIONS_CSV),
            mav_stations: StationTable::from_csv(MAV_STATIONS_CSV),
            renfe_stations: StationTable::from_csv(RENFE_STATIONS_CSV),
            eurostar_stations: StationTable::from_csv(EUROSTAR_STATIONS_CSV),
            exchange_rates: tariff::exchange_rates(TARIFFS_TOML)?,
            booking_windows: tariff::windows_from_toml(BOOKING_WINDOWS_TOML)?,
            residence_journeys_each_way: rules.residence.outbound_journeys,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_data_files_load() {
        let data = FareData::bundled().unwrap();
        assert_eq!(data.passes.passes.len(), 10);
        assert_eq!(data.residence_journeys_each_way, 1);
        assert!(data.reservations.rules.len() > 30);
    }
}
