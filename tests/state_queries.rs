//! Tests for the state-query accessors: `is_started`, `is_shutdown`, and
//! `get_current_network_name`.
//!
//! These cover the documented post-conditions across the full lifecycle:
//! construct -> start -> shutdown -> restart, plus the `bool`/`Result` value
//! contracts of each accessor.

mod common;

use net_kit::Net;

/// A freshly constructed `Net` reports not started / shut down.
#[test]
fn fresh_net_reports_not_started() {
    let net = Net::new();
    assert!(!net.is_started(), "a new Net has not been started");
    assert!(net.is_shutdown(), "a new Net is in the shutdown state");
    // is_started and is_shutdown are always strict inverses of each other.
    assert_ne!(net.is_started(), net.is_shutdown());
}

/// `Net::default()` matches `Net::new()` for the state queries.
#[test]
fn default_reports_not_started() {
    let net = Net::default();
    assert!(!net.is_started());
    assert!(net.is_shutdown());
}

/// After `start()` the accessors flip to started; after `shutdown()` they flip
/// back. The two queries stay exact inverses throughout.
#[test]
fn started_then_shutdown_toggles_state() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async { net.start().await.expect("start should succeed") });
    assert!(net.is_started(), "started after start()");
    assert!(!net.is_shutdown(), "not shut down after start()");
    assert_ne!(net.is_started(), net.is_shutdown());

    net.shutdown().expect("shutdown should succeed");
    assert!(!net.is_started(), "not started after shutdown()");
    assert!(net.is_shutdown(), "shut down after shutdown()");
    assert_ne!(net.is_started(), net.is_shutdown());
}

/// A redundant second `start()` leaves the state queries reporting started.
#[test]
fn redundant_start_keeps_started() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async {
        net.start().await.unwrap();
        net.start().await.expect("second start is a no-op Ok");
    });

    assert!(net.is_started());
    assert!(!net.is_shutdown());

    net.shutdown().unwrap();
}

/// `shutdown` without a prior `start` is a no-op: the Net stays shut down.
#[test]
fn shutdown_without_start_stays_shutdown() {
    let net = Net::new();
    net.shutdown().expect("shutdown on unstarted Net is Ok");
    assert!(!net.is_started());
    assert!(net.is_shutdown());
}

/// A double `shutdown` leaves the queries reporting shut down.
#[test]
fn double_shutdown_stays_shutdown() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async { net.start().await.unwrap() });
    net.shutdown().expect("first shutdown ok");
    net.shutdown().expect("second shutdown is a no-op Ok");

    assert!(!net.is_started());
    assert!(net.is_shutdown());
}

/// The state queries track every step of a start -> shutdown -> restart cycle.
#[test]
fn state_queries_across_full_lifecycle() {
    let rt = common::build_runtime();
    let net = Net::new();

    // Initial: not started.
    assert!(net.is_shutdown());

    // Several start/shutdown cycles must keep the invariant intact.
    for _ in 0..5 {
        rt.block_on(async { net.start().await.unwrap() });
        assert!(net.is_started());
        assert!(!net.is_shutdown());

        net.shutdown().unwrap();
        assert!(!net.is_started());
        assert!(net.is_shutdown());
    }
}

/// Before start, the current network name is unresolvable and reports
/// `Ok(None)` rather than erroring.
#[test]
fn network_name_is_none_before_start() {
    let net = Net::new();
    assert_eq!(
        net.get_current_network_name().unwrap(),
        None,
        "no network name before start"
    );
}

/// After start, querying the network name must not error. The concrete value
/// depends on the host (and platform): on Windows it may be `Some(ssid)` or a
/// connected-network name; on other platforms it is `None`. Either way the call
/// succeeds and any returned name is non-empty.
#[test]
fn network_name_after_start_does_not_error() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let name = net
        .get_current_network_name()
        .expect("querying network name must not error after start");

    if let Some(name) = name.as_ref() {
        assert!(
            !name.trim().is_empty(),
            "a resolved network name is never blank"
        );
    }

    net.shutdown().unwrap();
}

/// After shutdown the network name reverts to `Ok(None)`, matching the
/// not-started contract.
#[test]
fn network_name_is_none_after_shutdown() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async { net.start().await.unwrap() });
    net.shutdown().unwrap();

    assert_eq!(
        net.get_current_network_name().unwrap(),
        None,
        "no network name once shut down again"
    );
}

/// Calling `get_current_network_name` repeatedly is stable and never errors.
#[test]
fn network_name_repeated_queries_are_stable() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    for _ in 0..5 {
        net.get_current_network_name()
            .expect("repeated network-name queries must not error");
    }

    net.shutdown().unwrap();
}
