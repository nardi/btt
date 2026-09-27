//! Access to the text files of a GTFS feed ZIP.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;
use thiserror::Error;
use zip::ZipArchive;

use crate::database::{ZipReading, sql_identifier, sql_string};

/// Errors from reading a feed ZIP.
#[derive(Debug, Error)]
pub enum FeedArchiveError {
    /// The ZIP could not be opened or read.
    #[error("could not read {path}: {source}")]
    Zip {
        /// The ZIP file.
        path: String,
        /// Why reading failed.
        source: zip::result::ZipError,
    },
    /// A file could not be read or unpacked.
    #[error("could not read {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// Why reading failed.
        source: io::Error,
    },
}

/// A feed ZIP with the names and header lines of its files.
pub struct FeedArchive {
    /// Path of the ZIP.
    zip_path: PathBuf,
    /// Path inside the ZIP of each file, keyed by its file name, such as `stops.txt`.
    entry_paths: FxHashMap<String, String>,
}

impl FeedArchive {
    /// Opens a feed ZIP and lists its files. Files in a subdirectory count by their name.
    ///
    /// # Errors
    ///
    /// Returns an error when the ZIP cannot be read.
    pub fn open(zip_path: &Path) -> Result<Self, FeedArchiveError> {
        let zip_error = |source| FeedArchiveError::Zip {
            path: zip_path.display().to_string(),
            source,
        };
        let file = File::open(zip_path).map_err(|source| FeedArchiveError::Io {
            path: zip_path.display().to_string(),
            source,
        })?;
        let archive = ZipArchive::new(file).map_err(zip_error)?;
        let entry_paths = archive
            .file_names()
            .map(|entry_path| {
                let file_name = entry_path.rsplit('/').next().unwrap_or(entry_path);
                (file_name.to_ascii_lowercase(), entry_path.to_owned())
            })
            .collect();
        Ok(Self {
            zip_path: zip_path.to_owned(),
            entry_paths,
        })
    }

    /// Returns whether the feed has a file.
    pub fn has(&self, file_name: &str) -> bool {
        self.entry_paths.contains_key(file_name)
    }

    /// Returns the column names from the first line of a file, or an empty list when the feed
    /// has no such file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read.
    pub fn columns(&self, file_name: &str) -> Result<Vec<String>, FeedArchiveError> {
        let Some(entry_path) = self.entry_paths.get(file_name) else {
            return Ok(Vec::new());
        };
        let io_error = |source| FeedArchiveError::Io {
            path: format!("{}:{entry_path}", self.zip_path.display()),
            source,
        };
        let mut archive = self.archive()?;
        let entry = archive
            .by_name(entry_path)
            .map_err(|source| FeedArchiveError::Zip {
                path: self.zip_path.display().to_string(),
                source,
            })?;
        let mut header = String::new();
        BufReader::new(entry)
            .read_line(&mut header)
            .map_err(io_error)?;
        Ok(header
            .trim_start_matches('\u{feff}')
            .trim_end()
            .split(',')
            .map(|column| column.trim().trim_matches('"').to_owned())
            .collect())
    }

    /// Returns a SQL string literal naming a file of the feed for `read_csv`.
    ///
    /// With `zipfs`, DuckDB reads the file inside the ZIP. Otherwise the file is unpacked into
    /// a directory named after the ZIP, once.
    ///
    /// # Errors
    ///
    /// Returns an error when unpacking fails.
    pub fn csv_location(
        &self,
        file_name: &str,
        zip_reading: &ZipReading,
    ) -> Result<String, FeedArchiveError> {
        let entry_path = self
            .entry_paths
            .get(file_name)
            .map_or(file_name, String::as_str);
        match zip_reading {
            ZipReading::Zipfs => {
                let zip_path = self.zip_path.display().to_string().replace('\\', "/");
                Ok(sql_string(&format!("zip://{zip_path}/{entry_path}")))
            }
            ZipReading::Unpacked(directory) => {
                let unpacked = self.unpack(entry_path, directory)?;
                Ok(sql_string(
                    &unpacked.display().to_string().replace('\\', "/"),
                ))
            }
        }
    }

    /// Unpacks one file unless an unpacked copy of the same size exists.
    fn unpack(&self, entry_path: &str, directory: &Path) -> Result<PathBuf, FeedArchiveError> {
        let zip_name = self
            .zip_path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned());
        let target_directory = directory.join(zip_name.unwrap_or_default());
        let target = target_directory.join(entry_path.rsplit('/').next().unwrap_or(entry_path));
        let io_error = |source| FeedArchiveError::Io {
            path: target.display().to_string(),
            source,
        };
        let mut archive = self.archive()?;
        let mut entry = archive
            .by_name(entry_path)
            .map_err(|source| FeedArchiveError::Zip {
                path: self.zip_path.display().to_string(),
                source,
            })?;
        if fs::metadata(&target).is_ok_and(|metadata| metadata.len() == entry.size()) {
            return Ok(target);
        }
        fs::create_dir_all(&target_directory).map_err(io_error)?;
        let mut file = File::create(&target).map_err(io_error)?;
        io::copy(&mut entry, &mut file).map_err(io_error)?;
        Ok(target)
    }

    /// Opens the ZIP for reading one entry.
    fn archive(&self) -> Result<ZipArchive<File>, FeedArchiveError> {
        let path = self.zip_path.display().to_string();
        let file = File::open(&self.zip_path).map_err(|source| FeedArchiveError::Io {
            path: path.clone(),
            source,
        })?;
        ZipArchive::new(file).map_err(|source| FeedArchiveError::Zip { path, source })
    }
}

/// Returns a SQL expression for a CSV column, or `NULL` when the file lacks it. GTFS makes many
/// columns optional, and SQL cannot name a column that does not exist.
pub fn column_or_null(columns: &[String], name: &str) -> String {
    if columns.iter().any(|column| column == name) {
        sql_identifier(name)
    } else {
        "NULL".to_owned()
    }
}
