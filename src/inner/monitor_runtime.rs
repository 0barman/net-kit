use tokio::sync::{oneshot, watch};

/// Runtime handle for the monitor task created by a single `start` call.
pub(crate) struct MonitorRuntime {
    /// Used to signal the monitor task to stop.
    pub(crate) stop_sender: oneshot::Sender<()>,
    /// Set once the monitor task has obtained its initial state; `start` waits
    /// for this to become ready before returning.
    pub(crate) initial_state: watch::Receiver<bool>,
}
