//! Shared helpers for the `Net` integration tests.
//!
//! All comments in the test suite are written in English on purpose.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Runtime;

/// Build a fresh multi-threaded Tokio runtime for a single test.
///
/// Each test owns its own runtime so that tests stay isolated and can be run
/// in parallel without sharing scheduler state.
pub fn build_runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for test")
}

/// Build a single-threaded Tokio runtime, used by tests that intentionally
/// exercise the external-runtime entry point under a minimal scheduler.
pub fn build_current_thread_runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build current-thread tokio runtime for test")
}

/// A short, generous wait used when we want background callbacks (dispatched on
/// the engine callback thread pool) to have a chance to run before asserting.
pub const CALLBACK_SETTLE: Duration = Duration::from_millis(200);

/// A no-op listener factory whose invocations are counted through the supplied
/// atomic counter. Returned as the boxed listener type expected by `register`.
pub fn counting_listener(
    counter: Arc<std::sync::atomic::AtomicUsize>,
) -> net_kit::NetworkStatusListener {
    Box::new(move |_status| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    })
}
