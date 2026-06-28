use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use n0_watcher::Watcher as _;
use tokio::runtime::Handle;
use tokio::sync::{oneshot, watch};
use tokio::time::{self, MissedTickBehavior};
use vibe_ready::{log_s, VibeEngine, VibeEngineConfig, VibeLogListener};

use crate::ip_stack::IpStack;
use crate::net::{NetworkStatusListener, NetworkStatusListenerHandle};
use crate::net_error::NetError;
use crate::network_status::NetworkStatus;

use super::monitor_runtime::MonitorRuntime;
use super::monitor_state::{Dispatcher, MonitorState, SharedListener};

pub(crate) struct InnerNet {
    engine: VibeEngine,
    state: Arc<Mutex<MonitorState>>,
    /// Tokio runtime handle entered while a listener callback runs.
    ///
    /// Listener callbacks are dispatched on vibe-ready's callback thread pool,
    /// whose workers are bare OS threads with **no ambient Tokio runtime**. A
    /// listener that calls `tokio::spawn` (or any other runtime-dependent API)
    /// would otherwise panic with *"there is no reactor running, must be called
    /// from the context of a Tokio 1.x runtime"*. We capture a handle here and
    /// enter it for the duration of every callback so such listeners resolve
    /// against this runtime instead of crashing. `None` when no runtime could
    /// be captured; the callback is still panic-guarded in that case.
    runtime_handle: Option<Handle>,
}

impl InnerNet {
    /// Create the engine using the Tokio runtime bundled with (and owned by)
    /// `net-kit` via vibe-ready.
    pub fn new() -> Result<InnerNet, NetError> {
        let config = VibeEngineConfig::builder()
            .app_name("net-kit")
            .runtime_worker_threads(3)
            .callback_threads(2)
            .queue_capacity(1024, 256)
            .build();

        let engine = VibeEngine::create(config).map_err(NetError::from)?;

        Ok(InnerNet {
            engine,
            state: Arc::new(Mutex::new(MonitorState::default())),
            // `start` is async, so construction runs inside the caller's
            // runtime; capture that handle so listener callbacks (which execute
            // on vibe-ready's runtime-less callback threads) can still spawn.
            runtime_handle: Handle::try_current().ok(),
        })
    }

    /// Create the engine using a Tokio runtime supplied by the developer. The
    /// caller is responsible for keeping that runtime alive; `shutdown` only
    /// destroys engine resources and does not close this runtime.
    pub fn new_with_tokio_rt(runtime_handle: Handle) -> Result<InnerNet, NetError> {
        let config = VibeEngineConfig::builder().app_name("net-kit").build();
        let engine = VibeEngine::create_with_runtime_handle(config, runtime_handle.clone())
            .map_err(NetError::from)?;

        Ok(InnerNet {
            engine,
            state: Arc::new(Mutex::new(MonitorState::default())),
            // The developer-supplied runtime is the one their listener
            // callbacks expect to spawn onto; enter it during every callback.
            runtime_handle: Some(runtime_handle),
        })
    }

    /// Lock the instance state. Returns [`NetError::Lock`] on poison; never
    /// panics.
    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, MonitorState>, NetError> {
        self.state.lock().map_err(NetError::from_poison)
    }

    /// Start network monitoring. Redundant calls are ignored; `start` may be
    /// called again after `shutdown`.
    ///
    /// Synchronous part: if not already monitoring, post the monitor task on the
    /// engine and return the initial-state receiver; the caller then awaits
    /// [`InnerNet::wait_for_initial_state`] outside the lock.
    pub fn begin(&self) -> Result<watch::Receiver<bool>, NetError> {
        let mut state = self.lock_state()?;
        if let Some(monitor) = state.monitor.as_ref() {
            // Already monitoring; reuse its initial-state receiver.
            Ok(monitor.initial_state.clone())
        } else {
            Ok(self.spawn_monitor_task(&mut state))
        }
    }

    /// Post the long-lived monitor task on the engine's async lane and return
    /// the initial-state receiver.
    fn spawn_monitor_task(&self, state: &mut MonitorState) -> watch::Receiver<bool> {
        let (stop_sender, stop_receiver) = oneshot::channel();
        let (initial_state_sender, initial_state) = watch::channel(false);

        let shared_state = Arc::clone(&self.state);
        let dispatcher = self.dispatcher();

        self.engine.post(Self::monitor_until_stopped(
            shared_state,
            dispatcher,
            stop_receiver,
            initial_state_sender,
        ));

        state.monitor = Some(MonitorRuntime {
            stop_sender,
            initial_state: initial_state.clone(),
        });
        initial_state
    }

    /// Build a dispatcher that delivers a single listener callback to the
    /// engine callback thread pool.
    fn dispatcher(&self) -> Dispatcher {
        let callback = self.engine.executor().callback();
        let runtime_handle = self.runtime_handle.clone();
        Arc::new(move |listener: SharedListener, status: NetworkStatus| {
            let runtime_handle = runtime_handle.clone();
            callback.execute(move || {
                Self::invoke_listener(runtime_handle.as_ref(), &listener, status);
            });
        })
    }

    /// Execute a single registered listener on the engine callback thread pool,
    /// guaranteeing the call can never crash the process.
    ///
    /// The callback pool runs on bare OS worker threads (vibe-ready hands work
    /// to a `threadpool` worker) that have **no ambient Tokio runtime**, so a
    /// listener calling a runtime-dependent API such as `tokio::spawn` would
    /// otherwise panic with *"there is no reactor running, must be called from
    /// the context of a Tokio 1.x runtime"* and unwind straight through the
    /// worker. Two independent layers of protection are applied:
    ///
    /// 1. When a runtime [`Handle`] is available it is entered for the duration
    ///    of the call, so the listener's `tokio::spawn` / timers resolve against
    ///    that runtime instead of panicking.
    /// 2. The call is wrapped in [`catch_unwind`], so *any* panic raised by
    ///    third-party listener code (for any reason, not only a missing runtime,
    ///    and even if the entered runtime has since shut down) is contained and
    ///    turned into a logged, non-fatal event.
    fn invoke_listener(
        runtime_handle: Option<&Handle>,
        listener: &SharedListener,
        status: NetworkStatus,
    ) {
        // Entering the handle installs it as this thread's current runtime for
        // the lifetime of `_runtime_guard`; dropping the guard restores the
        // previous (empty) context.
        let _runtime_guard = runtime_handle.map(Handle::enter);
        if catch_unwind(AssertUnwindSafe(|| listener(status))).is_err() {
            log_s!("network_status_listener", "listener_panic", status.name());
        }
    }

    pub async fn wait_for_initial_state(mut initial_state: watch::Receiver<bool>) {
        while !*initial_state.borrow_and_update() {
            if initial_state.changed().await.is_err() {
                break;
            }
        }
    }

    /// Stop network monitoring and destroy the engine, releasing all resources
    /// created by `start`. Redundant calls are ignored.
    ///
    /// The engine is destroyed on a best-effort basis even if an internal lock
    /// is poisoned; the lock error is returned to the caller, but engine
    /// resources are still released, keeping the logic self-contained.
    pub fn shutdown(&self) -> Result<(), NetError> {
        // Collect the result of the lock operations first, but destroy the
        // engine regardless of success or failure.
        let lock_result = (|| -> Result<(), NetError> {
            let monitor = self.lock_state()?.monitor.take();
            if let Some(monitor) = monitor {
                let _ = monitor.stop_sender.send(());
            }
            // Silently reset the state; no callbacks are fired.
            {
                let mut guard = self.lock_state()?;
                guard.reachability = NetworkStatus::Unavailable;
                guard.ip_stack = IpStack::None;
            }
            Ok(())
        })();

        // Destroy the engine: this cancels the monitor task on the async lane
        // and reclaims runtime resources. A developer-supplied external runtime
        // is not closed (see vibe-ready semantics). Destroy runs regardless of
        // whether the lock operations above failed, ensuring resources are
        // released.
        self.engine.destroy(|_result| {});

        lock_result
    }

    /// Query whether the network is currently available. Returns
    /// [`NetError::Lock`] on poison.
    pub fn local_network_reachability(&self) -> Result<NetworkStatus, NetError> {
        #[cfg(target_os = "windows")]
        if let Some(reachability) = Self::windows_network_reachability() {
            self.update_reachability(reachability)?;
            return Ok(reachability);
        }

        Ok(self.lock_state()?.reachability)
    }

    /// Query the current IP-stack capability. Returns the cached value tracked
    /// by the monitor task; [`NetError::Lock`] on poison.
    pub fn ip_stack(&self) -> Result<IpStack, NetError> {
        Ok(self.lock_state()?.ip_stack)
    }

    /// Register a network notification listener; multiple may be registered.
    /// Callbacks are executed on the engine callback thread pool.
    pub fn register(
        &self,
        listener: NetworkStatusListener,
    ) -> Result<NetworkStatusListenerHandle, NetError> {
        let mut state = self.lock_state()?;
        let handle = state.next_listener_handle();
        state.listeners.insert(handle, Arc::from(listener));
        Ok(handle)
    }

    /// Unregister a network notification listener by handle. Returns
    /// [`NetError::Lock`] on poison.
    pub fn unregister(&self, handle: NetworkStatusListenerHandle) -> Result<bool, NetError> {
        Ok(self.lock_state()?.listeners.remove(&handle).is_some())
    }

    /// Clear all registered network listeners.
    pub fn clear_all_listener(&self) -> Result<(), NetError> {
        self.lock_state()?.listeners.clear();
        Ok(())
    }

    /// Query the name of the network the host is currently connected to.
    ///
    /// On Windows the connected Wi-Fi SSID is preferred, falling back to the
    /// `NetworkListManager` connected-network name. On other platforms no name
    /// is currently resolvable, so `Ok(None)` is returned. This call inspects
    /// the operating system directly and does not touch the instance lock, so
    /// it never returns [`NetError::Lock`]; the `Result` is kept for API
    /// symmetry with the other state-touching methods.
    pub fn get_current_network_name(&self) -> Result<Option<String>, NetError> {
        #[cfg(target_os = "windows")]
        {
            Ok(Self::query_current_network())
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(None)
        }
    }

    // ------------------------------------------------------------------
    // Monitor task and reachability computation
    // ------------------------------------------------------------------

    fn reachability_from_state(state: &netwatch::netmon::State) -> NetworkStatus {
        if state.default_route_interface.is_some() && (state.have_v4 || state.have_v6) {
            NetworkStatus::Available
        } else {
            NetworkStatus::Unavailable
        }
    }

    /// Derive the IP-stack capability from the `netwatch` interface state. This
    /// uses the same `have_v4` / `have_v6` flags on every platform, so the
    /// reported value has consistent cross-platform semantics.
    fn ip_stack_from_state(state: &netwatch::netmon::State) -> IpStack {
        IpStack::from_flags(state.have_v4, state.have_v6)
    }

    fn current_reachability(state: &netwatch::netmon::State) -> NetworkStatus {
        #[cfg(target_os = "windows")]
        if let Some(reachability) = Self::windows_network_reachability() {
            return reachability;
        }

        Self::reachability_from_state(state)
    }

    /// Update reachability and, when it changes, dispatch all listener
    /// callbacks on the callback thread pool.
    fn update_reachability(&self, reachability: NetworkStatus) -> Result<(), NetError> {
        Self::update_reachability_inner(&self.state, &self.dispatcher(), reachability)
    }

    /// Static version used by the monitor task (which only holds an
    /// `Arc<Mutex<MonitorState>>` and the dispatcher).
    ///
    /// Returns [`NetError::Lock`] on poison, leaving handling to the caller;
    /// never panics.
    fn update_reachability_inner(
        state: &Arc<Mutex<MonitorState>>,
        dispatcher: &Dispatcher,
        reachability: NetworkStatus,
    ) -> Result<(), NetError> {
        let listeners = {
            let mut guard = state.lock().map_err(NetError::from_poison)?;
            if guard.reachability == reachability {
                return Ok(());
            }
            guard.reachability = reachability;
            guard.listeners.values().cloned().collect::<Vec<_>>()
        };

        log_s!(
            "network_status_listener",
            "network_status",
            reachability.name()
        );

        for listener in listeners {
            dispatcher(listener, reachability);
        }
        Ok(())
    }

    /// Update both reachability and IP-stack capability under a single lock.
    ///
    /// The IP-stack value is always refreshed (it has no listeners and fires no
    /// callbacks). Reachability is only updated, and its listeners only
    /// dispatched, when it actually changes — preserving the existing
    /// change-detection contract. Used by the monitor task, which holds the full
    /// `netwatch` state needed to compute both values at once.
    ///
    /// Returns [`NetError::Lock`] on poison, leaving handling to the caller;
    /// never panics.
    fn update_state_inner(
        state: &Arc<Mutex<MonitorState>>,
        dispatcher: &Dispatcher,
        reachability: NetworkStatus,
        ip_stack: IpStack,
    ) -> Result<(), NetError> {
        let listeners = {
            let mut guard = state.lock().map_err(NetError::from_poison)?;
            guard.ip_stack = ip_stack;
            if guard.reachability == reachability {
                return Ok(());
            }
            guard.reachability = reachability;
            guard.listeners.values().cloned().collect::<Vec<_>>()
        };

        log_s!(
            "network_status_listener",
            "network_status",
            reachability.name()
        );

        for listener in listeners {
            dispatcher(listener, reachability);
        }
        Ok(())
    }

    /// Monitor task body: ported from the reference project's
    /// `monitor_until_stopped`, but the state comes from the instance rather
    /// than a global.
    ///
    /// This task runs on the engine's async lane and cannot return errors to
    /// the developer; if an internal lock becomes poisoned
    /// (`update_reachability_inner` returns `Err`), the task gracefully exits
    /// the loop and resets the state, and never panics.
    async fn monitor_until_stopped(
        state: Arc<Mutex<MonitorState>>,
        dispatcher: Dispatcher,
        mut stop_receiver: oneshot::Receiver<()>,
        initial_state: watch::Sender<bool>,
    ) {
        let Ok(monitor) = netwatch::netmon::Monitor::new().await else {
            let _ =
                Self::update_reachability_inner(&state, &dispatcher, NetworkStatus::Unavailable);
            let _ = initial_state.send(true);
            return;
        };
        let mut interface_state = monitor.interface_state();

        let initial = interface_state.get();
        let current = Self::current_reachability(&initial);
        let ip_stack = Self::ip_stack_from_state(&initial);
        // If the initial state update fails (lock poisoned), end the task.
        if Self::update_state_inner(&state, &dispatcher, current, ip_stack).is_err() {
            let _ = initial_state.send(true);
            return;
        }
        let _ = initial_state.send(true);

        let mut refresh_interval = time::interval(Duration::from_secs(2));
        refresh_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = &mut stop_receiver => break,
                update = interface_state.updated() => {
                    match update {
                        Ok(new_state) => {
                            let reachability = Self::current_reachability(&new_state);
                            let ip_stack = Self::ip_stack_from_state(&new_state);
                            // Exit the monitor loop if the lock is poisoned.
                            if Self::update_state_inner(&state, &dispatcher, reachability, ip_stack).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = refresh_interval.tick() => {
                    let snapshot = interface_state.get();
                    let reachability = Self::current_reachability(&snapshot);
                    let ip_stack = Self::ip_stack_from_state(&snapshot);
                    if Self::update_state_inner(&state, &dispatcher, reachability, ip_stack).is_err() {
                        break;
                    }
                }
            }
        }

        // The task is exiting; silently reset the state. If the lock is
        // poisoned it cannot be reset, so just give up (without panicking).
        if let Ok(mut guard) = state.lock() {
            guard.reachability = NetworkStatus::Unavailable;
            guard.ip_stack = IpStack::None;
        }
    }

    // ------------------------------------------------------------------
    // Windows network reachability (based on the NetworkListManager COM API)
    // ------------------------------------------------------------------

    #[cfg(target_os = "windows")]
    fn reachability_from_windows_connectivity(
        connectivity: windows::Win32::Networking::NetworkListManager::NLM_CONNECTIVITY,
    ) -> NetworkStatus {
        use windows::Win32::Networking::NetworkListManager::{
            NLM_CONNECTIVITY_IPV4_INTERNET, NLM_CONNECTIVITY_IPV6_INTERNET,
        };

        if connectivity.0 & NLM_CONNECTIVITY_IPV4_INTERNET.0 != 0
            || connectivity.0 & NLM_CONNECTIVITY_IPV6_INTERNET.0 != 0
        {
            NetworkStatus::Available
        } else {
            NetworkStatus::Unavailable
        }
    }

    #[cfg(target_os = "windows")]
    fn windows_network_reachability() -> Option<NetworkStatus> {
        use windows::Win32::{
            Networking::NetworkListManager::{INetworkListManager, NetworkListManager},
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
            },
        };

        unsafe {
            let com_initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let reachability = (|| {
                let manager: INetworkListManager =
                    CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
                let connectivity = manager.GetConnectivity().ok()?;
                Some(Self::reachability_from_windows_connectivity(connectivity))
            })();

            if com_initialized {
                CoUninitialize();
            }

            reachability
        }
    }

    // ------------------------------------------------------------------
    // Windows connected-network name (Wi-Fi SSID + NetworkListManager)
    // ------------------------------------------------------------------

    /// Resolve the name of the currently connected network on Windows.
    ///
    /// Prefers the active Wi-Fi SSID; if that is unavailable, falls back to the
    /// `NetworkListManager` connected-network name. Returns `None` when no
    /// connected network can be resolved.
    #[cfg(target_os = "windows")]
    fn query_current_network() -> Option<String> {
        Self::query_wifi_network().or_else(Self::query_windows_connected_network)
    }

    /// Query the active Wi-Fi SSID via `netsh wlan show interfaces`.
    #[cfg(target_os = "windows")]
    fn query_wifi_network() -> Option<String> {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        /// Avoid spawning a visible console window for the `netsh` child process.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let output = Command::new("netsh")
            .args(["wlan", "show", "interfaces"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_netsh_wifi_ssid(&stdout)
    }

    /// Extract the `SSID` value from `netsh wlan show interfaces` output,
    /// ignoring the `BSSID` line and any empty value.
    #[cfg(target_os = "windows")]
    fn parse_netsh_wifi_ssid(output: &str) -> Option<String> {
        output.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            if key.eq_ignore_ascii_case("SSID") && !key.eq_ignore_ascii_case("BSSID") {
                let name = value.trim();
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
            None
        })
    }

    /// Query the first connected network's name via the `NetworkListManager`
    /// COM API. COM is uninitialized on every exit path via an RAII guard, even
    /// on early returns.
    #[cfg(target_os = "windows")]
    fn query_windows_connected_network() -> Option<String> {
        use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
        use windows::Win32::Networking::NetworkListManager::{
            INetworkListManager, NetworkListManager, NLM_ENUM_NETWORK_CONNECTED,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        struct CoUninit(bool);
        impl Drop for CoUninit {
            fn drop(&mut self) {
                if self.0 {
                    unsafe { CoUninitialize() };
                }
            }
        }

        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let should_uninit = hr.is_ok();
        // `RPC_E_CHANGED_MODE` means COM is already initialized with a different
        // model on this thread; the existing apartment is reused and must not be
        // uninitialized here. Any other failure is fatal to this query.
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return None;
        }
        let _co_guard = CoUninit(should_uninit);

        unsafe {
            let nlm: INetworkListManager =
                CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
            let networks = nlm.GetNetworks(NLM_ENUM_NETWORK_CONNECTED).ok()?;
            let mut fetched = 0;
            let mut items = [None];
            networks.Next(&mut items, Some(&mut fetched)).ok()?;
            if fetched == 0 {
                return None;
            }

            let name = items[0].as_ref()?.GetName().ok()?.to_string();
            if name.trim().is_empty() {
                return None;
            }

            Some(name)
        }
    }

    /// Install (or clear, with `None`) a listener that receives the engine's
    /// internal log records. The developer's callback runs on the engine's
    /// logging path; we wrap it so a panic inside it can never unwind through
    /// that path and abort the process.
    pub fn set_log_listener(&self, listener: Option<VibeLogListener>) {
        self.engine
            .set_log_listener(listener.map(Self::guard_log_listener))
    }

    /// Wrap a developer-supplied log listener so any panic it raises is caught
    /// and discarded instead of propagating into vibe-ready's logging path.
    fn guard_log_listener(listener: VibeLogListener) -> VibeLogListener {
        Box::new(move |info| {
            let _ = catch_unwind(AssertUnwindSafe(|| listener(info)));
        })
    }
}

#[cfg(test)]
mod tests {
    //! Regression coverage for the reported production crash:
    //!
    //! ```text
    //! there is no reactor running, must be called from the context of a Tokio 1.x runtime
    //! ```
    //!
    //! It happened because a registered listener called `tokio::spawn`, and
    //! `net-kit` dispatched that listener on vibe-ready's callback thread pool —
    //! bare OS workers with no ambient Tokio runtime. These tests exercise
    //! [`InnerNet::invoke_listener`], the single choke point every listener call
    //! now flows through, on a non-runtime thread (exactly like a callback
    //! worker).
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Instant;

    /// Build a single-threaded multi-thread runtime to hand to `invoke_listener`.
    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("build test runtime")
    }

    /// Spin-wait (off any runtime) until `flag` is set or the deadline passes.
    fn wait_for(flag: &AtomicBool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !flag.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        flag.load(Ordering::SeqCst)
    }

    #[test]
    fn listener_calling_tokio_spawn_runs_instead_of_panicking_with_handle() {
        // The exact pattern that crashed: the listener calls `tokio::spawn`.
        // With a runtime handle entered, it must spawn successfully — no panic.
        let runtime = test_runtime();
        let handle = runtime.handle().clone();

        let ran = Arc::new(AtomicBool::new(false));
        let ran_in_task = Arc::clone(&ran);
        let listener: SharedListener = Arc::new(move |_status| {
            let ran = Arc::clone(&ran_in_task);
            tokio::spawn(async move {
                ran.store(true, Ordering::SeqCst);
            });
        });

        // Runs on the current (non-runtime) thread, just like a callback worker.
        InnerNet::invoke_listener(Some(&handle), &listener, NetworkStatus::Available);

        assert!(
            wait_for(&ran),
            "listener's tokio::spawn should have executed on the entered runtime"
        );
    }

    #[test]
    fn listener_calling_tokio_spawn_without_handle_is_contained() {
        // No runtime handle available: the listener's `tokio::spawn` panics with
        // the reported "there is no reactor running" message. `invoke_listener`
        // must swallow it so the callback worker (and the process) survives.
        let body_ran = Arc::new(AtomicBool::new(false));
        let body_ran_in_listener = Arc::clone(&body_ran);
        let listener: SharedListener = Arc::new(move |_status| {
            body_ran_in_listener.store(true, Ordering::SeqCst);
            // Panics: no ambient runtime on this thread.
            tokio::spawn(async {});
        });

        // Must return normally despite the listener panicking internally.
        InnerNet::invoke_listener(None, &listener, NetworkStatus::Available);

        assert!(
            body_ran.load(Ordering::SeqCst),
            "listener body must have been entered before the contained panic"
        );
    }

    #[test]
    fn arbitrary_listener_panic_is_contained() {
        // Any panic from third-party listener code — not just a missing runtime
        // — must be contained rather than unwinding through the callback worker.
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_listener = Arc::clone(&calls);
        let listener: SharedListener = Arc::new(move |_status| {
            calls_in_listener.fetch_add(1, Ordering::SeqCst);
            panic!("listener blew up");
        });

        InnerNet::invoke_listener(None, &listener, NetworkStatus::Available);
        InnerNet::invoke_listener(None, &listener, NetworkStatus::Unavailable);

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "both panicking invocations must have run and been contained"
        );
    }
}
