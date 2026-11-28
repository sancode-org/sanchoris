use leanfran::Nat;
use sanchoris_core::can_start_workspace;

#[test]
fn online_host_below_limit_can_start_workspace() {
    assert!(can_start_workspace(true, Nat::from(1u64), Nat::from(2u64)));
}

#[test]
fn host_at_limit_cannot_start_workspace() {
    assert!(!can_start_workspace(true, Nat::from(2u64), Nat::from(2u64)));
}

#[test]
fn offline_host_cannot_start_workspace() {
    assert!(!can_start_workspace(
        false,
        Nat::from(0u64),
        Nat::from(2u64)
    ));
}
