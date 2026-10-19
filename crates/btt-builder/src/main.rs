//! Command-line entry point of the timetable builder.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use btt_builder::config::{BuilderConfig, FeedFormat};
use btt_builder::database::WorkDatabase;
use btt_builder::download::{
    Downloader, TRAINLINE_STATIONS_URL, TRANSITOUS_FEED_CONFIG_URL, TRANSITOUS_MIRROR_URL,
};
use btt_builder::layout_pages::{
    LayoutRules, SitemapIndex, VAGONWEB_TRAIN_SITEMAP_URLS, attach_layout_pages, parse_sitemap,
};
use btt_builder::load::ValidityWindow;
use btt_builder::{BuildError, FeedInput, PipelineInputs, assemble, publish, report, run_pipeline};
use btt_timetable::LoadedTimetableBundle;
use chrono::{Datelike, NaiveDate, Utc};
use clap::Parser;

/// Pause between requests to vagonWEB, which asks for 15 seconds in its robots.txt.
const VAGONWEB_REQUEST_GAP: Duration = Duration::from_secs(15);

/// Builds a BTT timetable bundle from the GTFS feeds in the configuration.
#[derive(Debug, Parser)]
struct Arguments {
    /// Feed configuration.
    #[arg(long, default_value = "data/builder.toml")]
    config: PathBuf,
    /// Words to strip from station names before matching.
    #[arg(long, default_value = "data/station_name_stopwords.toml")]
    stopwords: PathBuf,
    /// How train labels map to the train pages of vagonWEB.
    #[arg(long, default_value = "data/vagonweb.toml")]
    vagonweb_rules: PathBuf,
    /// Leaves out the links to vagonWEB, so the build does not download its sitemaps.
    #[arg(long)]
    no_vagonweb: bool,
    /// Directory for downloads, the working database and unpacked feed files.
    #[arg(long, default_value = "build-cache")]
    cache_directory: PathBuf,
    /// Directory for the bundle and the report.
    #[arg(long, default_value = "build-cache/output")]
    output_directory: PathBuf,
    /// First day of the validity window. Defaults to today in UTC.
    #[arg(long)]
    start_date: Option<NaiveDate>,
    /// Keeps the working database after the build, for inspection with the DuckDB CLI.
    #[arg(long)]
    keep_work_db: bool,
    /// Unpacks feed files instead of reading them in place with the `zipfs` extension.
    #[arg(long)]
    no_zipfs: bool,
    /// Memory DuckDB may use before it spills to disk, such as `4GB`. Lower it when other
    /// programs need the memory. The default is 10GB.
    #[arg(long)]
    memory_limit: Option<String>,
    /// Compresses the bundle and writes its checksum and manifest entry next to it. The value is
    /// the address that holds the bundle releases, such as
    /// `https://github.com/owner/btt/releases/download`.
    #[arg(long)]
    download_base_url: Option<String>,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match build(&Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "build failed");
            ExitCode::FAILURE
        }
    }
}

/// Downloads the inputs, runs the pipeline and writes the bundle and the report.
fn build(arguments: &Arguments) -> Result<(), BuildError> {
    let started = Instant::now();
    let config = BuilderConfig::read(&arguments.config, &arguments.stopwords)?;
    let start = arguments
        .start_date
        .unwrap_or_else(|| Utc::now().date_naive());
    let window = ValidityWindow {
        start,
        days: config.validity_window_days,
    };
    let first_version_of_day = u32::try_from(start.year()).unwrap_or(0) * 1_000_000
        + start.month() * 10_000
        + start.day() * 100;
    let bundle_version = next_bundle_version(&arguments.output_directory, first_version_of_day);

    let downloader = Downloader::new(arguments.cache_directory.join("feeds"))?;
    let feeds = download_feeds(&downloader, &config)?;
    let trainline_stations_csv = downloader.fetch(TRAINLINE_STATIONS_URL, "stations.csv")?;
    let layout_page_sources = if arguments.no_vagonweb {
        None
    } else {
        read_layout_page_sources(arguments)?
    };

    let work_directory = arguments.cache_directory.join("work");
    std::fs::create_dir_all(&work_directory)?;
    std::fs::create_dir_all(&arguments.output_directory)?;
    let database_path = work_directory.join("build.duckdb");
    // Every build starts from an empty database.
    for stale in [
        database_path.clone(),
        work_directory.join("build.duckdb.wal"),
    ] {
        if stale.exists() {
            std::fs::remove_file(stale)?;
        }
    }
    let database = WorkDatabase::open(Some(&database_path), &work_directory, !arguments.no_zipfs)?;
    if let Some(limit) = &arguments.memory_limit {
        database.set_memory_limit(limit)?;
    }

    let inputs = PipelineInputs {
        feeds,
        trainline_stations_csv,
        station_name_stopwords: &config.station_name_stopwords,
        night_train_categories: &config.night_train_categories,
        window,
        bundle_version,
    };
    let mut outcome = run_pipeline(&database, &inputs)?;
    let layout_page_report = layout_page_sources
        .as_ref()
        .map(|(rules, index)| attach_layout_pages(&mut outcome.bundle, rules, index));

    let bundle_path = arguments
        .output_directory
        .join(format!("timetable-{bundle_version}.bundle"));
    LoadedTimetableBundle::write_bundle(&outcome.bundle, &bundle_path)?;
    assemble::write_station_map(&database, &bundle_path.with_extension("stations.csv"))?;
    // Reading the file back checks it with the same validation the app runs.
    let loaded = LoadedTimetableBundle::open(&bundle_path)?;
    let bundle_file_bytes = u64::try_from(loaded.byte_size()).unwrap_or(u64::MAX);
    let cached_reuses = downloader.cached_reuses();
    let config_feeds: Vec<_> = inputs.feeds.iter().map(|feed| feed.config).collect();
    report::write_report(
        &database,
        &config_feeds,
        &outcome.merge_counts,
        &outcome.bundle,
        bundle_file_bytes,
        &report::ReportSections {
            cached_reuses: &cached_reuses,
            layout_pages: layout_page_report.as_ref(),
        },
        &arguments.output_directory,
    )?;
    if let Some(download_base_url) = &arguments.download_base_url {
        let published = publish::write_published_files(&bundle_path, &loaded, download_base_url)?;
        tracing::info!(
            compressed_bundle = %published.compressed_bundle.display(),
            "compressed the bundle"
        );
    }
    drop(database);
    if !arguments.keep_work_db {
        std::fs::remove_file(&database_path)?;
    }
    tracing::info!(
        bundle = %bundle_path.display(),
        bytes = bundle_file_bytes,
        elapsed = ?started.elapsed(),
        "build finished"
    );
    Ok(())
}

/// Downloads the ZIP of every configured feed and the Transitous configuration of its country.
fn download_feeds<'config>(
    downloader: &Downloader,
    config: &'config BuilderConfig,
) -> Result<Vec<FeedInput<'config>>, BuildError> {
    let mut feeds = Vec::with_capacity(config.feeds.len());
    for feed in &config.feeds {
        let extension = match feed.format {
            FeedFormat::Gtfs => "gtfs",
            FeedFormat::Netex => "netex",
        };
        let zip_name = format!("{}.{extension}.zip", feed.mirror_name);
        let url = feed
            .source_url
            .clone()
            .unwrap_or_else(|| format!("{TRANSITOUS_MIRROR_URL}/{zip_name}"));
        let zip_path = downloader.fetch(&url, &zip_name)?;
        let country_file = format!("{}.json", feed.country());
        let country_config_path = downloader.fetch(
            &format!("{TRANSITOUS_FEED_CONFIG_URL}/{country_file}"),
            &format!("transitous-{country_file}"),
        )?;
        feeds.push(FeedInput {
            config: feed,
            zip_path,
            country_config_path,
        });
    }
    Ok(feeds)
}

/// Reads the rules for the train pages of vagonWEB and downloads its sitemaps. Returns `None`
/// when a sitemap is unavailable, because a bundle without the links is better than no bundle.
fn read_layout_page_sources(
    arguments: &Arguments,
) -> Result<Option<(LayoutRules, SitemapIndex)>, BuildError> {
    let rules = LayoutRules::read(&arguments.vagonweb_rules)?;
    // The site asks for a pause between requests, which this downloader keeps.
    let downloader = Downloader::with_request_gap(
        arguments.cache_directory.join("vagonweb"),
        VAGONWEB_REQUEST_GAP,
    )?;
    let mut trains = Vec::new();
    for (position, url) in VAGONWEB_TRAIN_SITEMAP_URLS.iter().enumerate() {
        let text = downloader
            .fetch(url, &format!("sitemap-{position}.xml"))
            .map_err(|error| error.to_string())
            .and_then(|path| std::fs::read_to_string(path).map_err(|error| error.to_string()));
        match text {
            Ok(text) => trains.extend(parse_sitemap(&text)),
            Err(error) => {
                tracing::warn!(url, %error, "no vagonWEB sitemap, building without layout links");
                return Ok(None);
            }
        }
    }
    Ok(Some((rules, SitemapIndex::new(trains))))
}

/// Returns the version for a new bundle built on a day: `first_version_of_day` plus one more
/// than the highest build number of that day already in the output directory. Each rebuild
/// then gets its own version, which keeps the app from reusing legs planned on an older one.
fn next_bundle_version(output_directory: &Path, first_version_of_day: u32) -> u32 {
    let Ok(entries) = std::fs::read_dir(output_directory) else {
        return first_version_of_day;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version: u32 = name
                .strip_prefix("timetable-")?
                .strip_suffix(".bundle")?
                .parse()
                .ok()?;
            // The last two digits count the builds of one day.
            (version / 100 == first_version_of_day / 100).then_some(version + 1)
        })
        .max()
        .unwrap_or(first_version_of_day)
}
