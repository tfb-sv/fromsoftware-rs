use std::sync::atomic::Ordering;

use crate::input::{INPUT_BLOCKER, InputFlags, dinput};

/// Girdi hook kurulum hatasi (ER-Lab 2026-10-08: ilhook::HookError yerine; paylasimli MinHook).
#[derive(Debug, Clone)]
pub struct HookError(pub String);

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HookError {}

pub(super) unsafe fn install() -> Result<(), HookError> {
    let r = unsafe { dinput::install() };
    if let Err(e) = &r {
        tracing::error!("input hooks not installed: {e}");
    }
    r
}

/// Returns true if any of `flags` are currently set in the input blocker
pub(crate) fn is_blocked(flags: InputFlags) -> bool {
    INPUT_BLOCKER.get().is_some_and(|b| {
        InputFlags::from_bits_retain(b.flags.load(Ordering::Relaxed)).intersects(flags)
    })
}
