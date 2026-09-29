//! Keeps the most recent log lines in memory, so a problem report can include them.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use tracing_subscriber::fmt::MakeWriter;

/// Number of log lines a report includes at most.
const KEPT_LOG_LINES: usize = 500;

/// The most recent log lines, shared between the logger and the report command.
#[derive(Clone, Default)]
pub struct RecentLogLines(Arc<Mutex<VecDeque<String>>>);

impl RecentLogLines {
    /// Returns the kept lines, oldest first, as one text.
    pub fn text(&self) -> String {
        let lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut text = String::new();
        for line in lines.iter() {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// Adds formatted log output, which can hold several lines, and drops the oldest lines
    /// beyond the limit.
    fn push(&self, output: &[u8]) {
        let mut lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for line in String::from_utf8_lossy(output).lines() {
            if lines.len() == KEPT_LOG_LINES {
                lines.pop_front();
            }
            lines.push_back(line.to_owned());
        }
    }
}

/// Receives the output of one log event for `RecentLogLines`.
pub struct RecentLogLinesWriter(RecentLogLines);

impl io::Write for RecentLogLinesWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.push(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for RecentLogLines {
    type Writer = RecentLogLinesWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RecentLogLinesWriter(self.clone())
    }
}
