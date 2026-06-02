//! Detailed listener registration / unregistration semantics.

mod common;

use net_kit::Net;

/// Handles are allocated monotonically and never collide across many
/// registrations within a single started session.
#[test]
fn handles_are_unique_across_many_registrations() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let mut handles = Vec::new();
    for _ in 0..1_000 {
        handles.push(net.register(Box::new(|_| {})).unwrap().unwrap());
    }

    // Every handle must be distinct.
    let mut sorted = handles.clone();
    sorted.sort_by_key(|h| format!("{h:?}"));
    sorted.dedup_by_key(|h| format!("{h:?}"));
    assert_eq!(
        sorted.len(),
        handles.len(),
        "all registration handles must be unique"
    );

    // Each handle unregisters exactly once.
    for h in handles {
        assert!(net.unregister(h).unwrap());
    }

    net.shutdown().unwrap();
}

/// Unregistering the same handle twice: first succeeds, second fails.
#[test]
fn double_unregister_returns_false_second_time() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let h = net.register(Box::new(|_| {})).unwrap().unwrap();
    assert!(net.unregister(h).unwrap());
    assert!(!net.unregister(h).unwrap());

    net.shutdown().unwrap();
}

/// Unregistering a handle that belongs to a different `Net` instance must fail
/// gracefully (return false), never panic or remove an unrelated listener.
#[test]
fn unregister_handle_from_other_instance_fails() {
    let rt = common::build_runtime();
    let net_a = Net::new();
    let net_b = Net::new();
    rt.block_on(async {
        net_a.start().await.unwrap();
        net_b.start().await.unwrap();
    });

    // Register on B, then attempt to unregister that handle on A.
    let h_b = net_b.register(Box::new(|_| {})).unwrap().unwrap();
    assert!(
        !net_a.unregister(h_b).unwrap(),
        "a handle from another instance must not resolve"
    );
    // B can still unregister its own handle.
    assert!(net_b.unregister(h_b).unwrap());

    net_a.shutdown().unwrap();
    net_b.shutdown().unwrap();
}

/// `clear_all_listener` removes every registration; subsequent unregister of
/// the cleared handles returns false, and a fresh registration still works.
#[test]
fn clear_then_register_again() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let handles: Vec<_> = (0..20)
        .map(|_| net.register(Box::new(|_| {})).unwrap().unwrap())
        .collect();
    net.clear_all_listener().unwrap();

    for h in &handles {
        assert!(
            !net.unregister(*h).unwrap(),
            "cleared handles no longer resolve"
        );
    }

    // Registration still functions after a clear.
    assert!(net.register(Box::new(|_| {})).unwrap().is_some());

    net.shutdown().unwrap();
}

/// Clearing an empty listener set is a benign no-op (still Ok while started).
#[test]
fn clear_when_empty_is_ok() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    net.clear_all_listener()
        .expect("clear with no listeners is Ok");
    net.clear_all_listener().expect("repeated clear is Ok");

    net.shutdown().unwrap();
}

/// Listeners registered before shutdown do not leak across a restart: a handle
/// from a previous session must not resolve in the new session.
#[test]
fn handles_do_not_survive_restart() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async { net.start().await.unwrap() });
    let old_handle = net.register(Box::new(|_| {})).unwrap().unwrap();
    net.shutdown().unwrap();

    rt.block_on(async { net.start().await.unwrap() });
    // The new session starts with a fresh listener table.
    assert!(
        !net.unregister(old_handle).unwrap(),
        "stale handle from previous session must not resolve"
    );

    net.shutdown().unwrap();
}
