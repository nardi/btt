//! Lookups that ignore the answers kept from earlier ones, for a traveller who asks again.
//!
//! A booking site's answer is kept for half an hour, so asking again at once would return the
//! same answer. A lookup that runs inside `with_refresh(true, ...)` reads nothing from the kept
//! answers of searches, and saves what it fetches in their place.

use std::future::Future;

tokio::task_local! {
    /// Whether the lookup of the current task ignores kept answers.
    static REFRESH: bool;
}

/// Runs a lookup that ignores kept answers when `refresh` is set.
pub async fn with_refresh<F: Future>(refresh: bool, lookup: F) -> F::Output {
    REFRESH.scope(refresh, lookup).await
}

/// Returns whether the lookup of the current task ignores the kept answer under a key.
///
/// The exchange rates and the addresses of services that a lookup reads stay, because they are
/// no answers to the search that the traveller asks again.
pub fn ignores(key: &str) -> bool {
    REFRESH.try_with(|refresh| *refresh).unwrap_or(false)
        && !key.starts_with("ecb:")
        && !key.ends_with("service-address")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_a_refreshing_lookup_ignores_the_answer_of_a_search() {
        assert!(!ignores("db:1:2"));
        with_refresh(true, async {
            assert!(ignores("db:1:2"));
            assert!(!ignores("ecb:daily"));
            assert!(!ignores("sbb:service-address"));
        })
        .await;
        with_refresh(false, async { assert!(!ignores("db:1:2")) }).await;
        assert!(!ignores("db:1:2"));
    }
}
