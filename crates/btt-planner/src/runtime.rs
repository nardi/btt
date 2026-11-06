//! Tasks, timers and blocking work for the planning session.
//!
//! Natively these run on Tokio. In the browser there are no threads, so tasks run on the event
//! loop of the page or worker and blocking work runs in place.

use std::future::Future;
use std::time::Duration;

use futures_util::future::{AbortHandle, Abortable};

/// A spawned task that can be stopped.
pub struct TaskHandle {
    /// Stops the task the next time it would run.
    abort_handle: AbortHandle,
}

impl TaskHandle {
    /// Stops the task. Work that already runs on a blocking thread finishes, and its result is
    /// dropped.
    pub fn abort(&self) {
        self.abort_handle.abort();
    }
}

/// Starts a task that runs alongside the caller.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn(task: impl Future<Output = ()> + Send + 'static) -> TaskHandle {
    let (abort_handle, registration) = AbortHandle::new_pair();
    let task = Abortable::new(task, registration);
    tokio::spawn(async move {
        // An aborted task ends with an error that nobody waits for.
        let _ = task.await;
    });
    TaskHandle { abort_handle }
}

/// Starts a task that runs alongside the caller. The browser runs every task on one thread, so
/// the task does not have to be `Send`, which its timers are not.
#[cfg(target_arch = "wasm32")]
pub fn spawn(task: impl Future<Output = ()> + 'static) -> TaskHandle {
    let (abort_handle, registration) = AbortHandle::new_pair();
    let task = Abortable::new(task, registration);
    wasm_bindgen_futures::spawn_local(async move {
        // An aborted task ends with an error that nobody waits for.
        let _ = task.await;
    });
    TaskHandle { abort_handle }
}

/// Waits for a length of time.
pub async fn sleep(duration: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(duration).await;
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::sleep(duration).await;
}

/// Work on a blocking thread panicked or was cancelled before it returned.
#[derive(Debug, Clone, Copy)]
pub struct BlockingWorkFailed;

/// Runs work that takes long enough to block an async task, such as a search of the timetable.
///
/// Natively the work runs on a thread of Tokio's blocking pool. In the browser it runs in place
/// after the event loop has had a turn, so messages that arrived meanwhile reach their handlers
/// before the work starts.
pub async fn run_blocking<T, F>(work: F) -> Result<T, BlockingWorkFailed>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|_| BlockingWorkFailed)
    }
    #[cfg(target_arch = "wasm32")]
    {
        gloo_timers::future::sleep(Duration::ZERO).await;
        Ok(work())
    }
}
