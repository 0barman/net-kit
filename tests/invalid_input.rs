//! Abnormal / invalid-input scenarios.
//!
//! The `Net` surface is strongly typed, so "invalid values" here means:
//! operating before start, using stale/foreign handles, and supplying
//! misbehaving listeners (ones that panic or block).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use net_kit::Net;

/// Every query/mutation on a not-started `Net` returns its defined
/// not-started result rather than panicking.
#[test]
fn operations_before_start_are_well_defined() {
    let net = Net::new();

    assert_eq!(
        net.local_network_reachability().unwrap(),
        net_kit::NetworkStatus::Unavailable
    );
    assert!(net.register(Box::new(|_| {})).unwrap().is_none());
    assert!(net.clear_all_listener().is_err());
    // shutdown before start is a benign no-op.
    assert!(net.shutdown().is_ok());
}

/// A handle obtained in a prior (now shut-down) session is "invalid" input for
/// the current state; unregister must reject it without side effects.
#[test]
fn stale_handle_after_shutdown_is_rejected() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async { net.start().await.unwrap() });
    let handle = net.register(Box::new(|_| {})).unwrap().unwrap();
    net.shutdown().unwrap();

    // The engine is gone; unregister of the stale handle returns false.
    assert!(!net.unregister(handle).unwrap());
}

/// A listener that panics must not poison the `Net`: because callbacks are
/// dispatched on the engine callback thread pool (isolated from the public
/// lock), the panic stays contained and later operations keep working.
///
/// We cannot deterministically force a real network-status change, so this test
/// focuses on the containment guarantee around the public API: registering a
/// panicking listener and then exercising the rest of the surface must remain
/// safe.
#[test]
fn panicking_listener_does_not_poison_api() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    // Register a listener that would panic if ever invoked.
    let panic_handle = net
        .register(Box::new(|_| panic!("listener intentionally panics")))
        .unwrap()
        .expect("register ok");

    // The public API must remain fully usable regardless.
    let normal = net
        .register(Box::new(|_| {}))
        .unwrap()
        .expect("second register ok");
    assert_ne!(panic_handle, normal);
    assert!(net.unregister(panic_handle).unwrap());
    assert!(net.unregister(normal).unwrap());
    let _ = net.local_network_reachability();
    net.clear_all_listener().unwrap();

    net.shutdown().expect("shutdown remains possible");
}

/// A listener that blocks for a while must not deadlock the public API, since
/// listeners run on the callback pool rather than under the public lock. We
/// register a slow listener and confirm that registration and other calls
/// return promptly.
#[test]
fn slow_listener_does_not_block_public_api() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let invoked = Arc::new(AtomicUsize::new(0));
    let invoked_clone = Arc::clone(&invoked);
    let slow = net
        .register(Box::new(move |_| {
            invoked_clone.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }))
        .unwrap()
        .unwrap();

    // These calls should not be gated on the (potential) listener execution.
    let _ = net.local_network_reachability();
    assert!(net.register(Box::new(|_| {})).unwrap().is_some());
    assert!(net.unregister(slow).unwrap());

    net.shutdown().unwrap();
}
