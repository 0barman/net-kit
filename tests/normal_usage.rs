//! Normal, expected-usage scenarios for `Net`.
//!
//! These cover the happy path: construct, start, query, register, unregister,
//! clear, shutdown, and the documented post-conditions of each.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use net_kit::{Net, NetworkStatus};

/// A freshly constructed `Net` is in the "not started" state: querying
/// reachability yields the default (`Unavailable`), and listener management
/// reflects that no inner runtime exists yet.
#[test]
fn fresh_net_is_not_started() {
    let net = Net::new();

    assert_eq!(
        net.local_network_reachability().unwrap(),
        NetworkStatus::Unavailable
    );
    // register before start cannot allocate a handle.
    assert!(net.register(Box::new(|_| {})).unwrap().is_none());
    // unregister of an arbitrary-but-impossible handle is a no-op false.
    // (We cannot fabricate a handle without registering, so we only assert the
    // not-started register contract here.)
    // clear_all_listener before start is an explicit NotStarted error.
    assert!(net.clear_all_listener().is_err());
}

/// `Net::default()` must behave identically to `Net::new()`.
#[test]
fn default_matches_new() {
    let net = Net::default();
    assert_eq!(
        net.local_network_reachability().unwrap(),
        NetworkStatus::Unavailable
    );
    assert!(net.register(Box::new(|_| {})).unwrap().is_none());
}

/// After `start()` the engine exists: registration succeeds and reachability
/// reports a concrete value (whatever the host network state happens to be).
#[test]
fn start_then_basic_operations() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async {
        net.start().await.expect("start should succeed");
    });

    // Once started, a listener can be registered and yields a handle.
    let handle = net
        .register(Box::new(|_status| {}))
        .unwrap()
        .expect("register after start returns a handle");

    // Reachability is a valid enum value; both variants are acceptable since it
    // depends on the host. We simply assert it does not panic and is one of the
    // two known states.
    let status = net.local_network_reachability().unwrap();
    assert!(matches!(
        status,
        NetworkStatus::Available | NetworkStatus::Unavailable
    ));

    // The handle we just got can be unregistered exactly once.
    assert!(net.unregister(handle).unwrap(), "first unregister succeeds");
    assert!(
        !net.unregister(handle).unwrap(),
        "second unregister is a no-op"
    );

    net.shutdown().expect("shutdown should succeed");
}

/// Registering several listeners returns distinct handles and clearing removes
/// them all.
#[test]
fn register_multiple_then_clear() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let h1 = net.register(Box::new(|_| {})).unwrap().unwrap();
    let h2 = net.register(Box::new(|_| {})).unwrap().unwrap();
    let h3 = net.register(Box::new(|_| {})).unwrap().unwrap();

    // Handles must be unique.
    assert_ne!(h1, h2);
    assert_ne!(h2, h3);
    assert_ne!(h1, h3);

    net.clear_all_listener().expect("clear after start is Ok");

    // After clearing, the previously valid handles no longer resolve.
    assert!(!net.unregister(h1).unwrap());
    assert!(!net.unregister(h2).unwrap());
    assert!(!net.unregister(h3).unwrap());

    net.shutdown().unwrap();
}

/// The full documented lifecycle: start -> use -> shutdown -> start again.
#[test]
fn lifecycle_start_shutdown_restart() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async { net.start().await.unwrap() });
    let h = net.register(Box::new(|_| {})).unwrap().unwrap();
    assert!(net.unregister(h).unwrap());
    net.shutdown().unwrap();

    // After shutdown the engine is gone again: register returns None.
    assert!(net.register(Box::new(|_| {})).unwrap().is_none());
    assert!(net.clear_all_listener().is_err());

    // Restarting must work and re-enable registration.
    rt.block_on(async { net.start().await.unwrap() });
    assert!(net.register(Box::new(|_| {})).unwrap().is_some());
    net.shutdown().unwrap();
}

/// Registering a listener does not, by itself, invoke it. The callback is only
/// dispatched on an actual status change, so with no change observed during the
/// settle window the counter must remain zero.
#[test]
fn registering_does_not_immediately_invoke() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let counter = Arc::new(AtomicUsize::new(0));
    let _h = net
        .register(common::counting_listener(Arc::clone(&counter)))
        .unwrap()
        .unwrap();

    // Give any erroneously-eager callback a chance to fire.
    std::thread::sleep(common::CALLBACK_SETTLE);

    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "listener must not fire purely on registration"
    );

    net.shutdown().unwrap();
}
