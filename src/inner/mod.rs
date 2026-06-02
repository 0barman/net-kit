//! Internal implementation module.
//!
//! The public API [`crate::net::Net`] only forwards to [`inner_net::InnerNet`];
//! all of the actual runtime and network-monitoring logic is encapsulated in
//! this module, keeping it isolated from the public surface.
pub(crate) mod inner_net;
pub(crate) mod monitor_runtime;
pub(crate) mod monitor_state;
