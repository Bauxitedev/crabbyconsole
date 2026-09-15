use std::sync::LazyLock;

use clap::Parser;
use crabbyconsole_misc::gd::cli::parse_cli_godot_args;
use godot::{prelude::*, register::GodotClass};

// CrabConsoleArgs::init() will trigger this lazy lock
pub static CC_ARGS: LazyLock<CrabConsoleArgs> = LazyLock::new(parse_cli_godot_args);

// Note - these consts you put in default_value_t only work for basic types, so e.g. Option<String> does not work.
// In that case, we just assume the default is None.

/// Note - we have a "data bundle" here, see:
/// <https://godot-rust.github.io/book/register/constructors.html#objects-without-a-base-field>
///
/// That means we don't need a `base` field, and we can skip `base = ...` since the default is `RefCounted`.
// Make sure you explicitly set `about` to avoid including doc comment in your --help output!
#[derive(Parser, Debug, Clone, GodotClass)]
#[class(base=RefCounted)]
#[command(version, about = "crabbyconsole_cli", long_about = None)]
#[command(no_binary_name = true, name = "--")] // Name = "--" is purely aesthetic, for better error messages.
pub struct CrabConsoleArgs {
    #[var]
    #[arg(long)] //default_value_t = DEFAULT_FOO)]
    pub foo: bool, // TODO in the future we can put cli args here, e.g. --crabbyconsole-run <cmd> or something
                   // (but make sure it runs in a delayed fashion to ensure there is enough time to enable lockdown mode)
}

#[godot_api]
impl CrabConsoleArgs {
    //...
}

#[godot_api]
impl IRefCounted for CrabConsoleArgs {
    /// This will be called by the closure created in `assign_bespoke_cli_type()`.
    fn init(_base: Base<RefCounted>) -> Self {
        // Note - this clones the InnerArgs, but it doesn't contain a lot of data, so should be fine
        CC_ARGS.clone() // NOTE - this also runs in the editor!
        // NOTE - the game says an instance of InnerArgs is being leaked... maybe because it's stored in the lazy cell?
    }
}

impl Default for CrabConsoleArgs {
    /// This default will be used if the cli args are missing.
    /// Note: GodotDefault is NOT the same as this Default.
    ///
    /// In the future we could use `#![feature(default_field_values)]` here.
    /// Unfortunately clap does not support it right now, so it gives a syntax error in the struct definition.
    /// More information:
    /// - <https://github.com/clap-rs/clap/issues/5839>
    /// - <https://doc.rust-lang.org/nightly/unstable-book/language-features/default-field-values.html>
    fn default() -> Self {
        Self { foo: true }
    }
}
