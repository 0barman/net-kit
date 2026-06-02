use std::collections::HashMap;
use std::sync::Arc;

use crate::net::NetworkStatusListenerHandle;
use crate::network_status::NetworkStatus;

use super::monitor_runtime::MonitorRuntime;

/// Listeners are stored internally as `Arc<dyn Fn ...>` so they can be cloned
/// out and executed on the engine callback thread pool.
pub(crate) type SharedListener = Arc<dyn Fn(NetworkStatus) + Send + Sync + 'static>;

/// Dispatcher that delivers a single listener callback to the engine callback
/// thread pool.
///
/// vibe-ready's callback-pool type is not re-exported at the crate root and so
/// cannot be named directly; therefore it is captured at `start` time into a
/// nameable boxed closure that the monitor task can use across threads.
pub(crate) type Dispatcher = Arc<dyn Fn(SharedListener, NetworkStatus) + Send + Sync + 'static>;

/// Mutable state owned by `InnerNet`. Each `InnerNet` instance owns its own
/// copy; no global statics are used.
#[derive(Default)]
pub(crate) struct MonitorState {
    /// Current network reachability.
    pub(crate) reachability: NetworkStatus,
    /// Registered listener callbacks.
    pub(crate) listeners: HashMap<NetworkStatusListenerHandle, SharedListener>,
    /// Monotonic counter for allocating listener handles.
    next_listener_id: u64,
    /// The running monitor task; `None` means not started.
    pub(crate) monitor: Option<MonitorRuntime>,
}

impl MonitorState {
    pub(crate) fn next_listener_handle(&mut self) -> NetworkStatusListenerHandle {
        loop {
            self.next_listener_id = self.next_listener_id.wrapping_add(1);
            if self.next_listener_id == 0 {
                continue;
            }

            let handle = NetworkStatusListenerHandle::from_raw(self.next_listener_id);
            if !self.listeners.contains_key(&handle) {
                return handle;
            }
        }
    }
}
