//! Shared test-only synchronization (the cwd-changing tests must not race).
use std::sync::Mutex;
pub static CWD_LOCK: Mutex<()> = Mutex::new(());
