//! Move / ownership scenarios. `Net` is a value type that can be moved into
//! closures, threads, containers, and back out, without disturbing its state.

mod common;

use std::sync::Arc;
use std::thread;

use net_kit::Net;

/// Moving a started `Net` into another function preserves its started state and
/// any registrations made beforehand.
#[test]
fn move_into_function_preserves_state() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });
    let handle = net.register(Box::new(|_| {})).unwrap().unwrap();

    // Move `net` by value into a helper that continues to use it.
    fn consume(net: Net, h: net_kit::NetworkStatusListenerHandle) -> Net {
        assert!(net.unregister(h).unwrap(), "handle still valid after move");
        assert!(net.register(Box::new(|_| {})).unwrap().is_some());
        net
    }

    let net = consume(net, handle);
    net.shutdown().unwrap();
}

/// A started `Net` can be moved into a separate thread and operated on there.
#[test]
fn move_into_thread() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let joined = thread::spawn(move || {
        // The moved-in `Net` is fully functional on the new thread.
        assert!(net.register(Box::new(|_| {})).unwrap().is_some());
        let _ = net.local_network_reachability();
        net.shutdown().unwrap();
    });

    joined.join().expect("moved Net thread must not panic");
}

/// Moving a `Net` through a container (Vec) and back does not change behavior.
#[test]
fn move_through_container() {
    let rt = common::build_runtime();
    let mut bag: Vec<Net> = Vec::new();
    bag.push(Net::new());

    let net = bag.pop().unwrap();
    rt.block_on(async { net.start().await.unwrap() });
    assert!(net.register(Box::new(|_| {})).unwrap().is_some());
    net.shutdown().unwrap();
}

/// Dropping a started `Net` without an explicit shutdown must not panic. The
/// engine's own destructor path handles cleanup; we simply assert the drop is
/// safe.
#[test]
fn drop_without_shutdown_is_safe() {
    let rt = common::build_runtime();
    {
        let net = Net::new();
        rt.block_on(async { net.start().await.unwrap() });
        let _ = net.register(Box::new(|_| {}));
        // `net` is dropped here without calling shutdown().
    }
    // If we reach this line the drop did not panic.

    // The runtime is still healthy and usable.
    assert_eq!(rt.block_on(async { 1 + 1 }), 2);
}

/// Multiple independent `Net` instances are fully isolated: operating on or
/// dropping one does not affect another.
#[test]
fn independent_instances_are_isolated() {
    let rt = common::build_runtime();
    let a = Arc::new(Net::new());
    let b = Net::new();

    rt.block_on(async {
        a.start().await.unwrap();
        b.start().await.unwrap();
    });

    let ha = a.register(Box::new(|_| {})).unwrap().unwrap();

    // Shutting down B must not affect A's registrations.
    b.shutdown().unwrap();
    assert!(a.unregister(ha).unwrap(), "A is unaffected by B's shutdown");

    a.shutdown().unwrap();
}
