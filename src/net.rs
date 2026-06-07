use std::sync::RwLock;

use tokio::runtime::Handle;
use vibe_ready::VibeLogListener;

use crate::inner::inner_net::InnerNet;
use crate::ip_stack::IpStack;
use crate::net_error::NetError;
use crate::network_status::NetworkStatus;

pub type NetworkStatusListener = Box<dyn Fn(NetworkStatus) + Send + Sync + 'static>;

/// Listener for the engine's internal diagnostic log records.
///
/// This is the log callback accepted by [`Net::set_log_listener`]; it is a
/// re-export of the underlying engine listener type.
pub type LogListener = VibeLogListener;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetworkStatusListenerHandle(u64);

impl NetworkStatusListenerHandle {
    pub(crate) fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

/// The single public API facade exposed by `net-kit`.
///
/// `Net` itself is only responsible for lifecycle management and forwarding;
/// all of the actual runtime and network-monitoring logic lives in the
/// internal [`InnerNet`], ensuring isolation.
pub struct Net {
    inner: RwLock<Option<InnerNet>>,
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl Net {
    /// Create an instance only; does not start network monitoring.
    pub fn new() -> Net {
        Net {
            inner: RwLock::new(None),
        }
    }

    /// Start network monitoring. Redundant calls are ignored; `start` may be
    /// called again after `shutdown`.
    ///
    /// Internally creates and owns a runtime engine owned by `net-kit`.
    pub async fn start(&self) -> Result<(), NetError> {
        self.start_inner(InnerNet::new).await
    }

    /// Start network monitoring. Redundant calls are ignored; `start` may be
    /// called again after `shutdown`.
    ///
    /// Uses a Tokio runtime supplied by the developer. The caller is
    /// responsible for keeping that runtime alive; `shutdown` does not close
    /// it.
    pub async fn start_with_tokio_rt(&self, runtime_handle: Handle) -> Result<(), NetError> {
        self.start_inner(move || InnerNet::new_with_tokio_rt(runtime_handle))
            .await
    }

    /// Install the internal engine, start monitoring, then wait for the initial
    /// network state to become ready.
    ///
    /// `factory` is only invoked when a new engine actually needs to be
    /// created — i.e. when not already started. This avoids needlessly creating
    /// an engine (and its owned runtime) on a redundant `start` only to drop it
    /// immediately; dropping an owned runtime inside an async context panics, so
    /// that path must never be taken.
    async fn start_inner<F>(&self, factory: F) -> Result<(), NetError>
    where
        F: FnOnce() -> Result<InnerNet, NetError>,
    {
        // Decide whether a new engine is needed while holding the write lock;
        // only invoke `factory` when not started, so no surplus engine is ever
        // created that would have to be dropped in an async context. Release the
        // lock as soon as the initial-state receiver is obtained, to avoid
        // holding it across an await.
        let initial_state = {
            let mut guard = self.inner.write().map_err(NetError::from_poison)?;
            if guard.is_none() {
                let candidate = factory()?;
                *guard = Some(candidate);
            }
            // Whether freshly installed or already present, trigger/reuse the
            // monitor on the current inner instance.
            match guard.as_ref() {
                Some(inner) => Some(inner.begin()?),
                None => None,
            }
        };

        if let Some(initial_state) = initial_state {
            InnerNet::wait_for_initial_state(initial_state).await;
        }

        Ok(())
    }

    /// Stop network monitoring and destroy all resources created by `start`.
    /// Redundant calls are ignored.
    pub fn shutdown(&self) -> Result<(), NetError> {
        let inner = self.inner.write().map_err(NetError::from_poison)?.take();
        if let Some(inner) = inner {
            inner.shutdown()?;
        }
        Ok(())
    }

    /// Query whether the network is currently available.
    ///
    /// Returns [`NetworkStatus::Unavailable`] when not started; returns
    /// [`NetError::Lock`] if an internal lock is poisoned, leaving recovery to
    /// the developer.
    pub fn local_network_reachability(&self) -> Result<NetworkStatus, NetError> {
        match self.inner.read().map_err(NetError::from_poison)?.as_ref() {
            Some(inner) => inner.local_network_reachability(),
            None => Ok(NetworkStatus::default()),
        }
    }

    /// Query the IP-stack capability currently available to the host.
    ///
    /// Reflects which IP protocol versions have usable addresses / routes
    /// (`have_v4` / `have_v6`), with identical semantics on every platform.
    /// Returns [`IpStack::None`] when not started; returns [`NetError::Lock`] if
    /// an internal lock is poisoned.
    pub fn ip_stack(&self) -> Result<IpStack, NetError> {
        match self.inner.read().map_err(NetError::from_poison)?.as_ref() {
            Some(inner) => inner.ip_stack(),
            None => Ok(IpStack::default()),
        }
    }

    /// Query whether IPv4 is currently available.
    ///
    /// Convenience for [`Net::ip_stack`] followed by [`IpStack::has_ipv4`].
    /// Returns `Ok(false)` when not started; returns [`NetError::Lock`] if an
    /// internal lock is poisoned.
    pub fn has_ipv4(&self) -> Result<bool, NetError> {
        Ok(self.ip_stack()?.has_ipv4())
    }

    /// Query whether IPv6 is currently available.
    ///
    /// Convenience for [`Net::ip_stack`] followed by [`IpStack::has_ipv6`].
    /// Returns `Ok(false)` when not started; returns [`NetError::Lock`] if an
    /// internal lock is poisoned.
    pub fn has_ipv6(&self) -> Result<bool, NetError> {
        Ok(self.ip_stack()?.has_ipv6())
    }

    /// Register a network notification listener; multiple may be registered.
    ///
    /// Returns `Ok(None)` when not started; `Ok(Some(handle))` on success after
    /// start; returns [`NetError::Lock`] if an internal lock is poisoned.
    pub fn register(
        &self,
        listener: NetworkStatusListener,
    ) -> Result<Option<NetworkStatusListenerHandle>, NetError> {
        match self.inner.read().map_err(NetError::from_poison)?.as_ref() {
            Some(inner) => inner.register(listener).map(Some),
            None => Ok(None),
        }
    }

    /// Unregister a network notification listener by handle.
    ///
    /// Returns `Ok(false)` when not started; `Ok(true)` when a listener was
    /// found and removed; returns [`NetError::Lock`] if an internal lock is
    /// poisoned.
    pub fn unregister(&self, handle: NetworkStatusListenerHandle) -> Result<bool, NetError> {
        match self.inner.read().map_err(NetError::from_poison)?.as_ref() {
            Some(inner) => inner.unregister(handle),
            None => Ok(false),
        }
    }

    /// Clear all registered network listeners. Returns [`NetError::NotStarted`]
    /// when not started.
    pub fn clear_all_listener(&self) -> Result<(), NetError> {
        match self.inner.read().map_err(NetError::from_poison)?.as_ref() {
            Some(inner) => inner.clear_all_listener(),
            None => Err(NetError::NotStarted),
        }
    }

    /// Query whether network monitoring is currently started.
    ///
    /// Returns `true` while an engine is installed (i.e. after `start` and
    /// before `shutdown`); returns `false` otherwise. A poisoned internal lock
    /// is treated as not started, so this method never panics and never returns
    /// an error.
    pub fn is_started(&self) -> bool {
        match self.inner.read() {
            Ok(guard) => guard.is_some(),
            Err(_) => false,
        }
    }

    /// Query whether network monitoring is currently shut down (i.e. not
    /// started).
    ///
    /// This is the logical inverse of [`Net::is_started`]: it returns `true`
    /// before the first `start` and after every `shutdown`. A poisoned internal
    /// lock is treated as not started, so this method never panics and never
    /// returns an error.
    pub fn is_shutdown(&self) -> bool {
        !self.is_started()
    }

    /// Query the name of the network the host is currently connected to.
    ///
    /// Returns `Ok(None)` when not started; `Ok(Some(name))` when started and a
    /// connected network name could be resolved; `Ok(None)` when started but no
    /// name is available (or the platform is unsupported); returns
    /// [`NetError::Lock`] if an internal lock is poisoned.
    pub fn get_current_network_name(&self) -> Result<Option<String>, NetError> {
        match self.inner.read().map_err(NetError::from_poison)?.as_ref() {
            Some(inner) => inner.get_current_network_name(),
            None => Ok(None),
        }
    }

    /// Install (or clear, with `None`) a listener for the engine's internal
    /// diagnostic log records.
    ///
    /// Returns [`NetError::NotStarted`] when not started; returns
    /// [`NetError::Lock`] if an internal lock is poisoned.
    pub fn set_log_listener(&self, listener: Option<LogListener>) -> Result<(), NetError> {
        match self.inner.read().map_err(NetError::from_poison)?.as_ref() {
            Some(inner) => {
                inner.set_log_listener(listener);
                Ok(())
            }
            None => Err(NetError::NotStarted),
        }
    }
}
