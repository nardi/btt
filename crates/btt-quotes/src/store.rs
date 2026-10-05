//! Where fetched quotes are kept between requests.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A place that keeps text documents under keys, for a limited time.
pub trait QuoteStore: Send + Sync {
    /// Returns the document under a key if it was saved less than `max_age` ago.
    fn load(&self, key: &str, max_age: Duration) -> Option<String>;

    /// Saves a document under a key, replacing an earlier one.
    fn save(&self, key: &str, document: &str);
}

/// A store in memory, which forgets everything when the program ends.
#[derive(Default)]
pub struct MemoryStore {
    /// The documents with the moment they were saved.
    documents: Mutex<HashMap<String, (Instant, String)>>,
}

impl QuoteStore for MemoryStore {
    fn load(&self, key: &str, max_age: Duration) -> Option<String> {
        let documents = self
            .documents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        documents
            .get(key)
            .filter(|(saved, _)| saved.elapsed() < max_age)
            .map(|(_, document)| document.clone())
    }

    fn save(&self, key: &str, document: &str) {
        self.documents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_owned(), (Instant::now(), document.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_document_loads_until_it_is_too_old() {
        let store = MemoryStore::default();
        store.save("a", "one");
        assert_eq!(
            store.load("a", Duration::from_secs(60)).as_deref(),
            Some("one")
        );
        assert_eq!(store.load("a", Duration::ZERO), None);
        assert_eq!(store.load("b", Duration::from_secs(60)), None);
    }
}
