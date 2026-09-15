use std::sync::OnceLock;

use color_eyre::eyre::{self, eyre};

/// `OnceLock<()>` is a way to encode a boolean that can only go `false -> true` at the type-level.
/// If this holds a `()`, lockdown mode is enabled.
/// It can never go back to false.
static LOCKDOWN: OnceLock<()> = OnceLock::new();

/// If lockdown mode is enabled, `GDScript` execution is disallowed, so only clap commands can be used.
/// This is recursive, so any clap commands that call `GDScript` will also fail.
/// E.g. `:set a 1+2` won't work, because `1+2` is also `GDScript`.
pub(super) fn is_lockdown_enabled() -> bool {
    LOCKDOWN.get().is_some()
}

pub(super) fn enable_lockdown() -> eyre::Result<()> {
    LOCKDOWN
        .set(())
        .map_err(|_| eyre!("lockdown mode was already enabled"))
}
