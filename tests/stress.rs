//! Stress / large-data scenarios. These push registration volume, churn, and
//! repeated lifecycle transitions to surface leaks, slowdowns, or handle-id
//! wraparound problems.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use net_kit::Net;

/// Register a very large number of listeners, then clear them all in one shot.
/// This exercises the listener map under high cardinality.
#[test]
fn massive_registration_then_clear() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    const COUNT: usize = 50_000;
    let mut handles = Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        handles.push(
            net.register(Box::new(|_| {}))
                .unwrap()
                .expect("register ok"),
        );
    }
    assert_eq!(handles.len(), COUNT);

    // A single clear must drop them all.
    net.clear_all_listener().unwrap();
    // Spot-check that a sample of the handles no longer resolve.
    for h in handles.iter().step_by(COUNT / 100) {
        assert!(!net.unregister(*h).unwrap());
    }

    net.shutdown().unwrap();
}

/// High churn: register/unregister in a tight loop many times. Verifies there
/// is no unbounded growth or handle reuse hazard within a session.
#[test]
fn high_churn_register_unregister() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    for _ in 0..100_000 {
        let h = net
            .register(Box::new(|_| {}))
            .unwrap()
            .expect("register ok");
        assert!(
            net.unregister(h).unwrap(),
            "freshly registered handle unregisters"
        );
    }

    net.shutdown().unwrap();
}

/// Many start/shutdown cycles to detect resource leaks or degradation across
/// repeated engine creation/teardown.
#[test]
fn many_lifecycle_cycles() {
    let rt = common::build_runtime();
    let net = Net::new();

    for i in 0..25 {
        rt.block_on(async {
            net.start()
                .await
                .unwrap_or_else(|e| panic!("start on cycle {i} failed: {e}"));
        });
        // Do a little work each cycle.
        let h = net.register(Box::new(|_| {})).unwrap().unwrap();
        let _ = net.local_network_reachability();
        assert!(net.unregister(h).unwrap());
        net.shutdown().unwrap();
    }
}

/// A large fan-out of listeners that all share one counter. We register many,
/// then drop them via clear, asserting the bookkeeping stays correct. The
/// counter is only here to ensure the closures capture real state (large
/// allocation footprint), not just zero-sized no-ops.
#[test]
fn large_stateful_listener_set() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let counter = Arc::new(AtomicUsize::new(0));
    const COUNT: usize = 10_000;
    let mut handles = Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        let c = Arc::clone(&counter);
        handles.push(
            net.register(Box::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap()
            .unwrap(),
        );
    }

    // Unregister half explicitly, clear the rest.
    for h in handles.iter().take(COUNT / 2) {
        assert!(net.unregister(*h).unwrap());
    }
    net.clear_all_listener().unwrap();
    for h in handles.iter().skip(COUNT / 2) {
        assert!(
            !net.unregister(*h).unwrap(),
            "remaining handles were cleared"
        );
    }

    net.shutdown().unwrap();
}
