use kou_tools::process::CancelToken;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[test]
fn cancellation_shares_the_owners_flag_without_a_watcher_thread() {
    let flag = Arc::new(AtomicBool::new(false));
    let token = CancelToken::from(flag.clone());
    assert!(!token.is_canceled());
    flag.store(true, Ordering::Release);
    assert!(token.is_canceled());
    let flag = Arc::new(AtomicBool::new(false));
    CancelToken::from(flag.clone()).cancel();
    assert!(flag.load(Ordering::Acquire));
}
