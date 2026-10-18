//! Builds BTT timetable bundles from GTFS feeds.
//!
//! DuckDB does the relational work: loading and filtering the feeds, matching stations and
//! computing footpaths. Rust resolves times to UTC, merges copies of the same train and
//! assembles the bundle.

pub mod assemble;
pub mod config;
pub mod database;
pub mod dedup;
pub mod download;
pub mod feed_archive;
pub mod load;
pub mod netex;
pub mod publish;
pub mod report;
pub mod resolve;
pub mod stations;

use std::path::PathBuf;
use std::time::Instant;

use btt_timetable::{BundleBuildError, TimetableBundle};
use thiserror::Error;

use crate::config::{FeedConfig, FeedFormat};
use crate::database::WorkDatabase;
use crate::dedup::MergeCounts;
use crate::feed_archive::FeedArchive;
use crate::load::ValidityWindow;

/// Errors from any build step.
#[derive(Debug, Error)]
pub enum BuildError {
    /// The configuration is invalid.
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    /// A download failed.
    #[error(transparent)]
    Download(#[from] download::DownloadError),
    /// A database statement failed.
    #[error(transparent)]
    Database(#[from] database::DatabaseError),
    /// A feed ZIP could not be read.
    #[error(transparent)]
    FeedArchive(#[from] feed_archive::FeedArchiveError),
    /// A NeTEx feed could not be read.
    #[error(transparent)]
    Netex(#[from] netex::NetexError),
    /// The bundle builder rejected the input.
    #[error("bundle: {0}")]
    BundleBuild(#[from] BundleBuildError),
    /// The bundle could not be written or read back.
    #[error(transparent)]
    BundleLoad(#[from] btt_timetable::BundleLoadError),
    /// A file could not be written.
    #[error("file: {0}")]
    Io(#[from] std::io::Error),
}

impl From<duckdb::Error> for BuildError {
    fn from(error: duckdb::Error) -> Self {
        Self::Database(database::DatabaseError::DuckDb(error))
    }
}

/// A feed ready to load: its configuration, its ZIP and its country's Transitous configuration.
pub struct FeedInput<'config> {
    /// How to read the feed.
    pub config: &'config FeedConfig,
    /// The downloaded feed ZIP.
    pub zip_path: PathBuf,
    /// The Transitous feed configuration of the feed's country, for its license.
    pub country_config_path: PathBuf,
}

/// Everything the pipeline reads.
pub struct PipelineInputs<'config> {
    /// The feeds to build from.
    pub feeds: Vec<FeedInput<'config>>,
    /// The Trainline stations dataset.
    pub trainline_stations_csv: PathBuf,
    /// Words stripped from station names before matching.
    pub station_name_stopwords: &'config [String],
    /// Train categories that stand for trains with couchettes or sleepers.
    pub night_train_categories: &'config [String],
    /// Days the bundle covers.
    pub window: ValidityWindow,
    /// Version of the data as `yyyymmddNN`.
    pub bundle_version: u32,
}

/// What the pipeline produces.
pub struct PipelineOutcome {
    /// The assembled bundle.
    pub bundle: TimetableBundle,
    /// Merges between each pair of feeds.
    pub merge_counts: MergeCounts,
}

/// Runs steps 2 to 9 in a working database that already has the schema.
///
/// # Errors
///
/// Returns the error of the first step that fails.
pub fn run_pipeline(
    database: &WorkDatabase,
    inputs: &PipelineInputs<'_>,
) -> Result<PipelineOutcome, BuildError> {
    database.run(include_str!("../sql/01_schema.sql"))?;
    for feed in &inputs.feeds {
        let started = Instant::now();
        match feed.config.format {
            FeedFormat::Gtfs => {
                let archive = FeedArchive::open(&feed.zip_path)?;
                load::load_feed(database, &archive, feed.config, inputs.window)?;
            }
            FeedFormat::Netex => {
                let trips =
                    netex::load_netex_feed(database, &feed.zip_path, feed.config, inputs.window)?;
                tracing::info!(feed = feed.config.mirror_name, trips, "read NeTEx feed");
            }
        }
        tracing::info!(feed = feed.config.mirror_name, elapsed = ?started.elapsed(), "loaded feed");
    }

    let started = Instant::now();
    stations::match_stations(
        database,
        &inputs.trainline_stations_csv,
        inputs.station_name_stopwords,
    )?;
    stations::compute_footpaths(database)?;
    tracing::info!(elapsed = ?started.elapsed(), "matched stations and computed footpaths");

    let started = Instant::now();
    let feed_configs: Vec<&FeedConfig> = inputs.feeds.iter().map(|feed| feed.config).collect();
    let resolved = resolve::resolve_trips(
        database,
        &feed_configs,
        inputs.night_train_categories,
        inputs.window,
    )?;
    tracing::info!(trips = resolved.len(), elapsed = ?started.elapsed(), "resolved trips to UTC");

    let started = Instant::now();
    let nearby = stations::nearby_station_pairs(database)?;
    let (mut trips, merge_counts) = dedup::merge_duplicate_trips(resolved, &nearby);
    let linked = dedup::link_berths(&mut trips, &nearby);
    let joined = dedup::join_continuations(&mut trips, &nearby);
    tracing::info!(
        trips = trips.len(),
        linked,
        joined,
        elapsed = ?started.elapsed(),
        "merged duplicate trips, linked split night trains and joined continuations"
    );

    let attributions = inputs
        .feeds
        .iter()
        .map(|feed| assemble::feed_attribution(database, feed.config, &feed.country_config_path))
        .collect();
    let started = Instant::now();
    let bundle = assemble::assemble_bundle(
        database,
        trips,
        attributions,
        inputs.window,
        inputs.bundle_version,
    )?;
    tracing::info!(elapsed = ?started.elapsed(), "assembled the bundle");
    Ok(PipelineOutcome {
        bundle,
        merge_counts,
    })
}
