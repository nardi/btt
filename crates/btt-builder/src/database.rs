//! The DuckDB working database the SQL steps run in.

use std::path::{Path, PathBuf};

use duckdb::Connection;
use thiserror::Error;

/// Errors from the working database.
#[derive(Debug, Error)]
pub enum DatabaseError {
    /// DuckDB reported an error.
    #[error("DuckDB: {0}")]
    DuckDb(#[from] duckdb::Error),
}

/// How the builder reads CSV files that sit inside a feed ZIP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZipReading {
    /// DuckDB reads them in place through the `zipfs` extension.
    Zipfs,
    /// The builder unpacks them into this directory first.
    Unpacked(PathBuf),
}

/// A DuckDB database for one build, with the settings the large loads need.
pub struct WorkDatabase {
    /// The open connection.
    pub connection: Connection,
    /// How CSV files inside ZIPs are read.
    pub zip_reading: ZipReading,
}

impl WorkDatabase {
    /// Opens a database file, or an in-memory database when `path` is `None`.
    ///
    /// `temp_directory` receives spilled data and, when `zipfs` is not available, unpacked
    /// feed files.
    ///
    /// # Errors
    ///
    /// Returns an error when DuckDB cannot open the database or apply the settings.
    pub fn open(
        path: Option<&Path>,
        temp_directory: &Path,
        allow_zipfs: bool,
    ) -> Result<Self, DatabaseError> {
        let connection = match path {
            Some(path) => Connection::open(path)?,
            None => Connection::open_in_memory()?,
        };
        connection.execute_batch(&format!(
            "SET memory_limit = '10GB';
             SET temp_directory = {};
             SET preserve_insertion_order = false;",
            sql_string(&temp_directory.display().to_string())
        ))?;
        let zip_reading = if allow_zipfs && load_zipfs(&connection) {
            ZipReading::Zipfs
        } else {
            ZipReading::Unpacked(temp_directory.join("unpacked"))
        };
        tracing::info!(?zip_reading, "opened the working database");
        Ok(Self {
            connection,
            zip_reading,
        })
    }

    /// Replaces the memory limit set on opening, such as `4GB`. DuckDB spills to the temporary
    /// directory beyond it, so a lower limit lets a build finish while other programs use the
    /// memory, at the cost of time.
    ///
    /// # Errors
    ///
    /// Returns an error when DuckDB rejects the limit.
    pub fn set_memory_limit(&self, limit: &str) -> Result<(), DatabaseError> {
        self.run(&format!("SET memory_limit = {};", sql_string(limit)))
    }

    /// Runs one or more SQL statements.
    ///
    /// # Errors
    ///
    /// Returns the DuckDB error of the first failing statement.
    pub fn run(&self, sql: &str) -> Result<(), DatabaseError> {
        self.connection.execute_batch(sql)?;
        Ok(())
    }

    /// Returns a single integer from a query, such as a count.
    ///
    /// # Errors
    ///
    /// Returns the DuckDB error when the query fails.
    pub fn count(&self, sql: &str) -> Result<i64, DatabaseError> {
        Ok(self.connection.query_row(sql, [], |row| row.get(0))?)
    }
}

/// Tries to install and load the community `zipfs` extension. Returns whether it worked.
///
/// The extension is optional. Without it the builder unpacks feed files, which is slower and
/// needs more disk space but gives the same result.
fn load_zipfs(connection: &Connection) -> bool {
    match connection.execute_batch("INSTALL zipfs FROM community; LOAD zipfs;") {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "zipfs is not available, feed files will be unpacked");
            false
        }
    }
}

/// Quotes text as a SQL string literal.
pub fn sql_string(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Quotes a name as a SQL identifier.
pub fn sql_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
