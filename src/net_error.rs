use std::fmt;

use vibe_ready::VibeEngineError;

/// Error type exposed by `net-kit`.
///
/// All internal [`VibeEngineError`]s are converted into this type so that the
/// underlying runtime details are not leaked to third-party callers.
#[derive(Debug, Clone)]
pub enum NetError {
    /// The monitor has not been started yet (call `Net::start` first).
    NotStarted,
    /// Failed to create the underlying runtime engine.
    EngineCreate(String),
    /// The runtime reported an execution error.
    Runtime(String),
    /// An internal lock is poisoned (a thread panicked while holding it, so the
    /// state may be inconsistent).
    ///
    /// This should not happen during normal operation. Once this error is
    /// returned, the caller is advised to call `Net::shutdown` to release
    /// resources and, if needed, `start` again to rebuild a clean instance.
    Lock,
}

impl NetError {
    /// Normalize any standard-library lock-poison error (`PoisonError`) into
    /// [`NetError::Lock`].
    ///
    /// Every `Mutex`/`RwLock` lock failure in the crate is converted through
    /// this function, which avoids crash-prone `.unwrap()` calls in product
    /// code and hands the error explicitly back to the developer.
    pub(crate) fn from_poison<T>(_err: std::sync::PoisonError<T>) -> Self {
        NetError::Lock
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::NotStarted => write!(f, "network monitor is not started"),
            NetError::EngineCreate(msg) => write!(f, "create runtime engine failed: {msg}"),
            NetError::Runtime(msg) => write!(f, "runtime error: {msg}"),
            NetError::Lock => write!(
                f,
                "internal state lock is poisoned; call shutdown and restart to recover"
            ),
        }
    }
}

impl std::error::Error for NetError {}

impl From<VibeEngineError> for NetError {
    fn from(err: VibeEngineError) -> Self {
        NetError::Runtime(err.to_string())
    }
}
