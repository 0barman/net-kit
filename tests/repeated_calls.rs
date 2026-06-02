//! Repeated / idempotent invocation scenarios: calling `start` and `shutdown`
//! more than once must be safe and well-defined.

mod common;

use net_kit::Net;

/// Calling `start` twice in a row must be ignored on the second call: the first
/// engine stays in place and the second call returns Ok without replacing it.
/// Registrations made after the first start must survive the redundant second
/// start.
#[test]
fn double_start_is_ignored_and_preserves_state() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async {
        net.start().await.expect("first start ok");
    });

    let handle = net
        .register(Box::new(|_| {}))
        .unwrap()
        .expect("registered after first start");

    rt.block_on(async {
        net.start().await.expect("second start is a no-op Ok");
    });

    // The handle registered before the redundant start must still be valid,
    // proving the second start did not replace the inner engine/state.
    assert!(
        net.unregister(handle).unwrap(),
        "listener survives a redundant start"
    );

    net.shutdown().unwrap();
}

/// Repeated `start` calls in quick succession (many times) remain stable.
#[test]
fn many_redundant_starts() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async {
        for i in 0..10 {
            net.start()
                .await
                .unwrap_or_else(|e| panic!("start #{i} failed: {e}"));
        }
    });

    assert!(net.register(Box::new(|_| {})).unwrap().is_some());
    net.shutdown().unwrap();
}

/// Calling `shutdown` without ever starting is a no-op and returns Ok.
#[test]
fn shutdown_without_start_is_ok() {
    let net = Net::new();
    net.shutdown().expect("shutdown on unstarted Net is Ok");
    // Still unstarted afterwards.
    assert!(net.register(Box::new(|_| {})).unwrap().is_none());
}

/// Calling `shutdown` twice is safe; the second call is a no-op.
#[test]
fn double_shutdown_is_ignored() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async { net.start().await.unwrap() });

    net.shutdown().expect("first shutdown ok");
    net.shutdown().expect("second shutdown is a no-op Ok");

    assert!(net.register(Box::new(|_| {})).unwrap().is_none());
}

/// A rapid start/shutdown alternation cycle must not corrupt state or leak the
/// started/stopped invariant.
#[test]
fn start_shutdown_alternation() {
    let rt = common::build_runtime();
    let net = Net::new();

    for _ in 0..5 {
        rt.block_on(async { net.start().await.unwrap() });
        assert!(
            net.register(Box::new(|_| {})).unwrap().is_some(),
            "registration works while started"
        );
        net.shutdown().unwrap();
        assert!(
            net.register(Box::new(|_| {})).unwrap().is_none(),
            "registration is disabled while stopped"
        );
    }
}
