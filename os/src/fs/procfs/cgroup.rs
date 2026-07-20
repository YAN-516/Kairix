#![allow(missing_docs)]

use alloc::string::String;

/// Kairix currently has no active cgroup hierarchy. Linux still exposes the
/// per-process membership file in that configuration, with no membership
/// records.
pub fn content() -> String {
    String::new()
}
