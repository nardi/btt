//! Downloads feeds, their Transitous configuration and the Trainline station list into a cache.

use std::fs;
use std::io::{self, Seek};
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Mirror of the GTFS feeds Transitous uses, already cleaned by its import scripts.
pub const TRANSITOUS_MIRROR_URL: &str = "https://api.transitous.org/gtfs";

/// Feed configuration of Transitous, one JSON file per country.
pub const TRANSITOUS_FEED_CONFIG_URL: &str =
    "https://raw.githubusercontent.com/public-transport/transitous/main/feeds";

/// Station list of the Trainline stations dataset.
pub const TRAINLINE_STATIONS_URL: &str =
    "https://raw.githubusercontent.com/trainline-eu/stations/master/stations.csv";

/// `User-Agent` for every request, which public servers ask clients to set.
const USER_AGENT: &str = concat!(
    "btt-builder/",
    env!("CARGO_PKG_VERSION"),
    " (BigTrainTrip timetable builder)"
);

/// Errors from downloading.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// The request failed or the server answered with an error status.
    #[error("download of {url} failed: {source}")]
    Request {
        /// The requested URL.
        url: String,
        /// Why it failed.
        source: reqwest::Error,
    },
    /// The file could not be written.
    #[error("could not write {path}: {source}")]
    Write {
        /// The file.
        path: String,
        /// Why writing failed.
        source: io::Error,
    },
}

/// Downloads files into a cache directory and skips files that have not changed.
pub struct Downloader {
    /// HTTP client with the builder's user agent.
    http_client: reqwest::blocking::Client,
    /// Directory that holds the cached files.
    cache_directory: PathBuf,
}

impl Downloader {
    /// Creates a downloader that caches into a directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created or the HTTP client fails to build.
    pub fn new(cache_directory: PathBuf) -> Result<Self, DownloadError> {
        fs::create_dir_all(&cache_directory).map_err(|source| DownloadError::Write {
            path: cache_directory.display().to_string(),
            source,
        })?;
        let http_client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            // Feeds can be several hundred megabytes, so only the connection gets a timeout.
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(None)
            .build()
            .map_err(|source| DownloadError::Request {
                url: String::new(),
                source,
            })?;
        Ok(Self {
            http_client,
            cache_directory,
        })
    }

    /// Returns the cached copy of a URL, downloading it when it is missing or the server has a
    /// newer version.
    ///
    /// The `Last-Modified` or `ETag` header of the cached copy is kept next to the file. A
    /// server that sends neither causes a download on every run.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails and no cached copy exists, or when writing fails.
    pub fn fetch(&self, url: &str, file_name: &str) -> Result<PathBuf, DownloadError> {
        let path = self.cache_directory.join(file_name);
        let stamp_path = self
            .cache_directory
            .join(format!("{file_name}.version-stamp"));
        let cached_stamp = fs::read_to_string(&stamp_path).ok();
        let remote_stamp = self.remote_version_stamp(url);
        if path.exists() {
            match &remote_stamp {
                Ok(Some(remote)) if cached_stamp.as_deref() == Some(remote.as_str()) => {
                    return Ok(path);
                }
                // Without a way to compare, a copy that exists is used as is.
                Err(error) => {
                    tracing::warn!(url, %error, "could not check for a newer version, using the cache");
                    return Ok(path);
                }
                Ok(_) => {}
            }
        }
        tracing::info!(url, "downloading");
        let remote_stamp = remote_stamp.ok().flatten();
        match self.download(url, &path, remote_stamp.as_deref()) {
            Ok(()) => {}
            // An older copy still builds a bundle, which beats no bundle at all.
            Err(error) if path.exists() => {
                tracing::warn!(url, %error, "could not download the newer version, using the cache");
                return Ok(path);
            }
            Err(error) => return Err(error),
        }
        if let Some(remote) = remote_stamp {
            write_file(&stamp_path, remote.as_bytes())?;
        }
        Ok(path)
    }

    /// Returns the `Last-Modified` header of a URL, or its `ETag` when the server sends no date.
    fn remote_version_stamp(&self, url: &str) -> Result<Option<String>, reqwest::Error> {
        let response = self.http_client.head(url).send()?.error_for_status()?;
        Ok(response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .or_else(|| response.headers().get(reqwest::header::ETAG))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned))
    }

    /// Streams a URL into a file, through a temporary file so an interrupted download never
    /// replaces a good copy.
    ///
    /// A connection that breaks off is resumed where it stopped, with a `Range` request. The
    /// `If-Range` header carries `version_stamp`, so a server whose file changed in between
    /// sends the whole new file instead of the rest of the old one.
    fn download(
        &self,
        url: &str,
        path: &Path,
        version_stamp: Option<&str>,
    ) -> Result<(), DownloadError> {
        let request_error = |source| DownloadError::Request {
            url: url.to_owned(),
            source,
        };
        let partial_path = path.with_extension("partial");
        let write_error = |source| DownloadError::Write {
            path: path.display().to_string(),
            source,
        };
        let mut file = fs::File::create(&partial_path).map_err(write_error)?;
        let mut received_bytes: u64 = 0;
        let mut attempt = 1;
        loop {
            let mut request = self.http_client.get(url);
            if received_bytes > 0 {
                request =
                    request.header(reqwest::header::RANGE, format!("bytes={received_bytes}-"));
                if let Some(stamp) = version_stamp {
                    request = request.header(reqwest::header::IF_RANGE, stamp);
                }
            }
            let copied = request
                .send()
                .and_then(reqwest::blocking::Response::error_for_status)
                .map_err(request_error)
                .and_then(|mut response| {
                    // A full answer to a range request means the server ignored the range or
                    // the file changed, so the download starts over.
                    if received_bytes > 0
                        && response.status() != reqwest::StatusCode::PARTIAL_CONTENT
                    {
                        file.set_len(0).map_err(write_error)?;
                        file.seek(io::SeekFrom::Start(0)).map_err(write_error)?;
                        received_bytes = 0;
                    }
                    let mut counting = CountingWriter {
                        inner: &mut file,
                        written_bytes: &mut received_bytes,
                    };
                    response.copy_to(&mut counting).map_err(request_error)
                });
            match copied {
                Ok(_) => break,
                Err(error) if attempt < MAXIMUM_DOWNLOAD_ATTEMPTS => {
                    tracing::warn!(url, %error, received_bytes, attempt, "download broke off, resuming");
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
        drop(file);
        fs::rename(&partial_path, path).map_err(write_error)?;
        Ok(())
    }
}

/// How often a download that breaks off is tried again before the builder gives up on it.
const MAXIMUM_DOWNLOAD_ATTEMPTS: u32 = 5;

/// Passes writes on to a file and counts the bytes, so a broken download knows where to resume.
struct CountingWriter<'a> {
    /// The file that receives the bytes.
    inner: &'a mut fs::File,
    /// Bytes written so far, including those of earlier attempts.
    written_bytes: &'a mut u64,
}

impl io::Write for CountingWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        *self.written_bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Writes a small file, naming it in the error.
fn write_file(path: &Path, contents: &[u8]) -> Result<(), DownloadError> {
    fs::write(path, contents).map_err(|source| DownloadError::Write {
        path: path.display().to_string(),
        source,
    })
}
