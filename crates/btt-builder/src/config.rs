//! The builder configuration, parsed and checked before any download starts.

use std::path::Path;

use regex::Regex;
use serde::Deserialize;
use thiserror::Error;

/// Errors from reading the configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// The file.
        path: String,
        /// Why reading failed.
        source: std::io::Error,
    },
    /// The file is not valid TOML or does not match the expected layout.
    #[error("invalid configuration: {0}")]
    Parse(#[from] toml::de::Error),
    /// A regular expression does not compile.
    #[error("invalid pattern in feed {feed}: {source}")]
    Pattern {
        /// The feed whose pattern failed.
        feed: String,
        /// Why the pattern failed.
        source: regex::Error,
    },
    /// A train label pattern has no `number` group.
    #[error("the train label pattern of feed {0} has no `number` group")]
    MissingNumberGroup(String),
    /// The validity window is empty or too long.
    #[error("validity_window_days must be between 1 and 128, not {0}")]
    InvalidValidityWindow(u16),
}

/// The whole configuration.
#[derive(Debug)]
pub struct BuilderConfig {
    /// Length of the bundle's validity window, starting on the build date.
    pub validity_window_days: u16,
    /// Feeds to build from.
    pub feeds: Vec<FeedConfig>,
    /// Words stripped from station names before comparing them, such as "hbf".
    pub station_name_stopwords: Vec<String>,
}

/// How to read one feed.
#[derive(Debug)]
pub struct FeedConfig {
    /// Name of the feed on the Transitous mirror, such as `at_Railway-Current-Reference-Data-2026`.
    /// The part before the first underscore is the country file of the Transitous feed
    /// configuration, the rest is the source name in it.
    pub mirror_name: String,
    /// How to find the station a stop belongs to.
    pub station_key: StationKeyRule,
    /// Which official station code the stops carry.
    pub station_code: FeedStationCodeScheme,
    /// How to read category and number of a train.
    pub train_label: TrainLabelRule,
}

impl FeedConfig {
    /// Returns the country part of the mirror name, such as `at`.
    pub fn country(&self) -> &str {
        self.mirror_name
            .split_once('_')
            .map_or("", |(country, _)| country)
    }

    /// Returns the source name part of the mirror name.
    pub fn source_name(&self) -> &str {
        self.mirror_name
            .split_once('_')
            .map_or(self.mirror_name.as_str(), |(_, name)| name)
    }
}

/// How to find the station a stop belongs to.
#[derive(Debug)]
pub enum StationKeyRule {
    /// Use `parent_station`, or the stop itself when it has none.
    ParentStation,
    /// Take the first capture group of a pattern over the stop ID, and fall back to
    /// `parent_station`. DHID-style IDs such as `de:05315:11201:7:71` need this, because their
    /// platforms often have no parent.
    StopIdPattern(String),
}

/// How a feed's stops carry an official station code, if at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedStationCodeScheme {
    /// The stops carry no code the Trainline dataset knows.
    NoStationCodes,
    /// The stop ID is a UIC station code.
    UicInStopId,
    /// The stop ID is an EVA number, as used by DB.
    EvaInStopId,
}

/// A GTFS field that can hold the category or number of a train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainLabelField {
    /// `trips.txt` `trip_short_name`.
    TripShortName,
    /// `routes.txt` `route_short_name`.
    RouteShortName,
    /// A product name derived from `routes.txt` `route_type`, such as "ICE" for 101.
    RouteType,
}

/// How to read category and number of a train from a trip.
#[derive(Debug)]
pub struct TrainLabelRule {
    /// Field the pattern reads.
    pub field: TrainLabelField,
    /// Pattern with a `number` group and an optional `category` group.
    pub pattern: Regex,
    /// Field that holds the category when the pattern has no `category` group or it does not
    /// match.
    pub category_field: TrainLabelField,
}

/// The configuration file as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderConfigFile {
    validity_window_days: u16,
    feed: Vec<FeedConfigFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedConfigFile {
    mirror_name: String,
    station_key_pattern: Option<String>,
    #[serde(default)]
    station_code: StationCodeFile,
    train_label: TrainLabelFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StationCodeFile {
    #[default]
    None,
    UicInStopId,
    EvaInStopId,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrainLabelFile {
    field: TrainLabelField,
    pattern: String,
    category_field: TrainLabelField,
}

/// The stopword file as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopwordFile {
    stopwords: Vec<String>,
}

impl BuilderConfig {
    /// Reads and checks the configuration and the stopword list.
    ///
    /// # Errors
    ///
    /// Returns an error when a file cannot be read, does not parse, or holds an invalid
    /// pattern or window.
    pub fn read(config_path: &Path, stopwords_path: &Path) -> Result<Self, ConfigError> {
        let file: BuilderConfigFile = toml::from_str(&read_text(config_path)?)?;
        let stopwords: StopwordFile = toml::from_str(&read_text(stopwords_path)?)?;
        if file.validity_window_days == 0 || file.validity_window_days > 128 {
            return Err(ConfigError::InvalidValidityWindow(
                file.validity_window_days,
            ));
        }
        let feeds = file
            .feed
            .into_iter()
            .map(FeedConfig::try_from)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            validity_window_days: file.validity_window_days,
            feeds,
            station_name_stopwords: stopwords.stopwords,
        })
    }
}

impl TryFrom<FeedConfigFile> for FeedConfig {
    type Error = ConfigError;

    fn try_from(file: FeedConfigFile) -> Result<Self, ConfigError> {
        let pattern_error = |source| ConfigError::Pattern {
            feed: file.mirror_name.clone(),
            source,
        };
        let station_key = match file.station_key_pattern {
            Some(pattern) => {
                // DuckDB checks the pattern again with RE2, which accepts the same syntax for
                // the simple patterns station keys need.
                Regex::new(&pattern).map_err(pattern_error)?;
                StationKeyRule::StopIdPattern(pattern)
            }
            None => StationKeyRule::ParentStation,
        };
        let pattern = Regex::new(&file.train_label.pattern).map_err(pattern_error)?;
        if !pattern
            .capture_names()
            .flatten()
            .any(|name| name == "number")
        {
            return Err(ConfigError::MissingNumberGroup(file.mirror_name));
        }
        Ok(Self {
            station_key,
            station_code: match file.station_code {
                StationCodeFile::None => FeedStationCodeScheme::NoStationCodes,
                StationCodeFile::UicInStopId => FeedStationCodeScheme::UicInStopId,
                StationCodeFile::EvaInStopId => FeedStationCodeScheme::EvaInStopId,
            },
            train_label: TrainLabelRule {
                field: file.train_label.field,
                pattern,
                category_field: file.train_label.category_field,
            },
            mirror_name: file.mirror_name,
        })
    }
}

/// Reads a text file, naming it in the error.
fn read_text(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.display().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(toml_text: &str) -> Result<FeedConfig, ConfigError> {
        let file: FeedConfigFile = toml::from_str(toml_text).unwrap();
        FeedConfig::try_from(file)
    }

    #[test]
    fn reads_a_feed_with_a_station_key_pattern() {
        let feed = feed(
            r#"
            mirror_name = "de_DELFI"
            station_key_pattern = '^(de:\d+:\d+)'
            [train_label]
            field = "trip_short_name"
            pattern = '^0*(?<number>\d+)$'
            category_field = "route_short_name"
            "#,
        )
        .unwrap();
        assert_eq!((feed.country(), feed.source_name()), ("de", "DELFI"));
        assert!(matches!(feed.station_key, StationKeyRule::StopIdPattern(_)));
        assert_eq!(feed.station_code, FeedStationCodeScheme::NoStationCodes);
    }

    #[test]
    fn rejects_a_label_pattern_without_number_group() {
        let result = feed(
            r#"
            mirror_name = "nl_OpenOV"
            [train_label]
            field = "trip_short_name"
            pattern = '^\d+$'
            category_field = "route_short_name"
            "#,
        );
        assert!(matches!(result, Err(ConfigError::MissingNumberGroup(_))));
    }
}
