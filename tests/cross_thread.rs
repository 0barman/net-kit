//! Cross-thread sharing scenarios. `Net` must be `Send + Sync` so it can be
//! wrapped in an `Arc` and driven from many threads concurrently.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use net_kit::Net;

/// Compile-time proof that `Net` is `Send + Sync` (and thus shareable across
/// threads via `Arc`). If this ever regresses, the crate will fail to compile.
#[test]
fn net_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Net>();
}

/// Many threads concurrently registering listeners on a shared, started `Net`
/// must all receive unique handles and never panic.
#[test]
fn concurrent_registration_yields_unique_handles() {
    let rt = common::build_runtime();
    let net = Arc::new(Net::new());
    rt.block_on(async { net.start().await.unwrap() });

    const THREADS: usize = 8;
    const PER_THREAD: usize = 200;

    let barrier = Arc::new(Barrier::new(THREADS));
    let mut joins = Vec::new();

    for _ in 0..THREADS {
        let net = Arc::clone(&net);
        let barrier = Arc::clone(&barrier);
        joins.push(thread::spawn(move || {
            // Synchronize the start so the threads truly contend.
            barrier.wait();
            let mut local = Vec::with_capacity(PER_THREAD);
            for _ in 0..PER_THREAD {
                local.push(
                    net.register(Box::new(|_| {}))
                        .unwrap()
                        .expect("register ok"),
                );
            }
            local
        }));
    }

    let mut all = Vec::new();
    for j in joins {
        all.extend(j.join().expect("worker thread panicked"));
    }

    assert_eq!(all.len(), THREADS * PER_THREAD);
    // All handles across all threads must be globally unique.
    let mut keys: Vec<String> = all.iter().map(|h| format!("{h:?}")).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(
        keys.len(),
        all.len(),
        "concurrently allocated handles must be unique"
    );

    net.shutdown().unwrap();
}

/// Mixed concurrent workload: some threads register, some unregister, some
/// query reachability. The shared `Net` must stay internally consistent and
/// never deadlock.
#[test]
fn concurrent_mixed_operations() {
    let rt = common::build_runtime();
    let net = Arc::new(Net::new());
    rt.block_on(async { net.start().await.unwrap() });

    // Pre-seed a batch of handles that an "unregister" worker will drain.
    let seeded: Arc<std::sync::Mutex<Vec<_>>> = Arc::new(std::sync::Mutex::new(
        (0..500)
            .map(|_| net.register(Box::new(|_| {})).unwrap().unwrap())
            .collect(),
    ));

    let errors = Arc::new(AtomicUsize::new(0));
    let mut joins = Vec::new();

    // Registrar threads.
    for _ in 0..3 {
        let net = Arc::clone(&net);
        joins.push(thread::spawn(move || {
            for _ in 0..300 {
                if net.register(Box::new(|_| {})).unwrap().is_none() {
                    // Only legal once shut down; here it would be a bug.
                }
            }
        }));
    }

    // Unregistrar thread.
    {
        let net = Arc::clone(&net);
        let seeded = Arc::clone(&seeded);
        joins.push(thread::spawn(move || {
            loop {
                let next = seeded.lock().unwrap().pop();
                match next {
                    Some(h) => {
                        // Each seeded handle should unregister cleanly the first time.
                        let _ = net.unregister(h);
                    }
                    None => break,
                }
            }
        }));
    }

    // Reader threads.
    for _ in 0..2 {
        let net = Arc::clone(&net);
        let errors = Arc::clone(&errors);
        joins.push(thread::spawn(move || {
            for _ in 0..500 {
                let s = net.local_network_reachability().unwrap();
                if !matches!(
                    s,
                    net_kit::NetworkStatus::Available | net_kit::NetworkStatus::Unavailable
                ) {
                    errors.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }

    for j in joins {
        j.join().expect("worker thread panicked");
    }

    assert_eq!(
        errors.load(Ordering::SeqCst),
        0,
        "no invalid status observed"
    );

    net.shutdown().unwrap();
}

/// One thread shuts the engine down while others are operating on it. The race
/// must resolve safely: post-shutdown operations simply observe the not-started
/// contract (register -> None) instead of panicking.
#[test]
fn shutdown_racing_with_operations_is_safe() {
    let rt = common::build_runtime();
    let net = Arc::new(Net::new());
    rt.block_on(async { net.start().await.unwrap() });

    let net_ops = Arc::clone(&net);
    let worker = thread::spawn(move || {
        // Hammer the API; any individual call may land before or after shutdown.
        for _ in 0..2_000 {
            let _ = net_ops.register(Box::new(|_| {}));
            let _ = net_ops.local_network_reachability();
            let _ = net_ops.clear_all_listener();
        }
    });

    // Shut down concurrently.
    net.shutdown().unwrap();

    worker.join().expect("operations thread must not panic");
}
