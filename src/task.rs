//! RAII asynchronous task wrappers with automatic cancellation on drop.

use std::future::Future;

use tokio::task::JoinHandle;

/// Tokio background task that aborts automatically when dropped.
///
/// Prevents background leaks by cancelling pending futures when a view or session drops.
pub struct Task<T>(JoinHandle<T>);

impl<T: Send + 'static> Task<T> {
    pub fn spawn(future: impl Future<Output = T> + Send + 'static) -> Self {
        Self(tokio::spawn(future))
    }
}

impl<T> Task<T> {
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl<T: Default> Task<T> {
    /// Awaits task completion, falling back to `T::default()` if the task panicked.
    pub async fn join(mut self) -> T {
        (&mut self.0).await.unwrap_or_default()
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Extracts and awaits the task in `slot` if it has completed, leaving running tasks in place.
pub async fn take_finished<T: Default>(slot: &mut Option<Task<T>>) -> Option<T> {
    if !slot.as_ref()?.is_finished() {
        return None;
    }
    Some(slot.take()?.join().await)
}
