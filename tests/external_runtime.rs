//! Scenarios exercising `start_with_tokio_rt`, where the caller supplies the
//! Tokio runtime instead of letting the library own one.

mod common;

use net_kit::{Net, NetworkStatus};

/// Starting with an externally-owned multi-threaded runtime works and behaves
/// like the owned-runtime path.
#[test]
fn start_with_external_multi_thread_runtime() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async {
        net.start_with_tokio_rt(tokio::runtime::Handle::current())
            .await
            .expect("start_with_tokio_rt should succeed");
    });

    assert!(matches!(
        net.local_network_reachability().unwrap(),
        NetworkStatus::Available | NetworkStatus::Unavailable
    ));
    assert!(net.register(Box::new(|_| {})).unwrap().is_some());

    net.shutdown().expect("shutdown should succeed");
}

/// The external runtime must outlive the engine but must NOT be shut down by
/// `Net::shutdown`. After shutting the engine down, the same runtime is still
/// usable for further work.
#[test]
fn external_runtime_survives_shutdown() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async {
        net.start_with_tokio_rt(tokio::runtime::Handle::current())
            .await
            .unwrap();
    });

    net.shutdown().unwrap();

    // The caller-owned runtime is still alive and can run a task.
    let answer = rt.block_on(async { 21 * 2 });
    assert_eq!(answer, 42, "external runtime remains usable after shutdown");
}

/// Restart on the external runtime after a shutdown is supported.
#[test]
fn external_runtime_restart() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async {
        net.start_with_tokio_rt(tokio::runtime::Handle::current())
            .await
            .unwrap();
    });
    net.shutdown().unwrap();

    rt.block_on(async {
        net.start_with_tokio_rt(tokio::runtime::Handle::current())
            .await
            .unwrap();
    });
    assert!(net.register(Box::new(|_| {})).unwrap().is_some());
    net.shutdown().unwrap();
}

/// A current-thread runtime is also a valid host for the external entry point.
#[test]
fn start_with_current_thread_runtime() {
    let rt = common::build_current_thread_runtime();
    let net = Net::new();

    rt.block_on(async {
        net.start_with_tokio_rt(tokio::runtime::Handle::current())
            .await
            .expect("current-thread runtime should also work");
        // Reachability is queryable while still inside the runtime context.
        let _ = net.local_network_reachability();
    });

    net.shutdown().unwrap();
}
