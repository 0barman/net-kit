//! Tests for the IP-stack capability accessors: `ip_stack`, `has_ipv4`, and
//! `has_ipv6`.
//!
//! These cover the documented post-conditions across the lifecycle (construct ->
//! start -> shutdown), the not-started value contract, and the internal
//! consistency between the enum query and the boolean convenience accessors.

mod common;

use net_kit::{IpStack, Net};

/// A freshly constructed `Net` reports no IP-stack capability and both
/// protocol accessors report `false`, without erroring.
#[test]
fn fresh_net_reports_none() {
    let net = Net::new();
    assert_eq!(net.ip_stack().unwrap(), IpStack::None);
    assert!(!net.has_ipv4().unwrap());
    assert!(!net.has_ipv6().unwrap());
}

/// `Net::default()` matches `Net::new()` for the IP-stack queries.
#[test]
fn default_reports_none() {
    let net = Net::default();
    assert_eq!(net.ip_stack().unwrap(), IpStack::None);
}

/// After start, querying the IP stack must not error. The concrete value
/// depends on the host's connectivity, but the boolean accessors must always be
/// internally consistent with the enum.
#[test]
fn ip_stack_after_start_is_consistent() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let stack = net
        .ip_stack()
        .expect("querying ip_stack must not error after start");

    assert_eq!(
        net.has_ipv4().unwrap(),
        stack.has_ipv4(),
        "has_ipv4 must agree with ip_stack"
    );
    assert_eq!(
        net.has_ipv6().unwrap(),
        stack.has_ipv6(),
        "has_ipv6 must agree with ip_stack"
    );

    net.shutdown().unwrap();
}

/// When the network is reachable, at least one IP protocol must be available,
/// since reachability itself requires `have_v4 || have_v6`.
#[test]
fn reachable_implies_some_ip_stack() {
    use net_kit::NetworkStatus;

    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    if net.local_network_reachability().unwrap() == NetworkStatus::Available {
        assert_ne!(
            net.ip_stack().unwrap(),
            IpStack::None,
            "a reachable network must expose at least one IP protocol"
        );
    }

    net.shutdown().unwrap();
}

/// After shutdown the IP stack reverts to `None`, matching the not-started
/// contract.
#[test]
fn ip_stack_is_none_after_shutdown() {
    let rt = common::build_runtime();
    let net = Net::new();

    rt.block_on(async { net.start().await.unwrap() });
    net.shutdown().unwrap();

    assert_eq!(net.ip_stack().unwrap(), IpStack::None);
    assert!(!net.has_ipv4().unwrap());
    assert!(!net.has_ipv6().unwrap());
}

/// Querying the IP stack repeatedly is stable and never errors.
#[test]
fn ip_stack_repeated_queries_are_stable() {
    let rt = common::build_runtime();
    let net = Net::new();
    rt.block_on(async { net.start().await.unwrap() });

    let first = net.ip_stack().unwrap();
    for _ in 0..5 {
        assert_eq!(
            net.ip_stack()
                .expect("repeated ip_stack queries must not error"),
            first,
            "ip_stack should be stable across back-to-back reads"
        );
    }

    net.shutdown().unwrap();
}
