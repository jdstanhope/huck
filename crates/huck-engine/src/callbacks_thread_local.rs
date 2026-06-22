//! Thread-local pointer to the active `Callbacks` for the in-flight builder
//! call. Used by the executor's builtin write hook (and Task 5's external
//! poll loop) to dispatch streaming line callbacks without threading the
//! Callbacks reference through every executor function.
//!
//! Pattern mirrors v206's `err_thread_local`. Sound because:
//!   1. Engine is `!Send + !Sync` — only one OS thread runs the executor.
//!   2. Pointer is installed for the synchronous duration of the wrapping
//!      `install` call; Guard's Drop clears even on panic.
//!   3. Callers materialize the writer in a tight scope and never store it
//!      across function boundaries.

use crate::exec_builder::Callbacks;
use std::cell::Cell;
use std::ptr::NonNull;

type CallbacksPtr = NonNull<Callbacks<'static>>;

thread_local! {
    static CALLBACKS_PTR: Cell<Option<CallbacksPtr>> = const { Cell::new(None) };
}

#[must_use = "guard must be held for the synchronous duration of the call"]
pub struct CallbacksGuard {
    prev: Option<CallbacksPtr>,
}

impl Drop for CallbacksGuard {
    fn drop(&mut self) {
        CALLBACKS_PTR.with(|c| c.set(self.prev));
    }
}

/// Install `callbacks` as the active thread-local for the returned guard's
/// lifetime. The guard's Drop restores the previous installation.
///
/// # Safety
/// The caller must hold `callbacks` alive for at least as long as the
/// returned guard. The pointer's lifetime is laundered to `'static`; the
/// guard's Drop clears it before `callbacks`'s actual lifetime ends.
pub(crate) unsafe fn install<'cb>(callbacks: &mut Callbacks<'cb>) -> CallbacksGuard {
    let raw: NonNull<Callbacks<'cb>> = NonNull::from(callbacks);
    // SAFETY: NonNull pointer transmute is safe because Callbacks<'cb> and
    // Callbacks<'static> have identical layout — `'cb` is a phantom-ish
    // lifetime on the boxed closures inside. The guard restores the prior
    // pointer on Drop, before `'cb` ends.
    let static_raw: NonNull<Callbacks<'static>> = unsafe { std::mem::transmute(raw) };
    let prev = CALLBACKS_PTR.with(|c| c.replace(Some(static_raw)));
    CallbacksGuard { prev }
}

/// Run `f` with the active Callbacks, if any. Returns whatever `f` returns;
/// passes `None` if no Callbacks is installed.
pub(crate) fn with_callbacks<R>(f: impl FnOnce(Option<&mut Callbacks<'_>>) -> R) -> R {
    CALLBACKS_PTR.with(|c| match c.get() {
        Some(mut p) => {
            // SAFETY: `install` guarantees `p` points to a valid Callbacks for
            // the duration of the guard, which encloses this call.
            f(Some(unsafe { p.as_mut() }))
        }
        None => f(None),
    })
}
