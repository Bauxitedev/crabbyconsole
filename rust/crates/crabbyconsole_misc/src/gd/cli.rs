use std::{any::type_name, fmt::Debug, sync::OnceLock};

use clap::Parser;
use godot::{
    classes::Os,
    obj::{Base, Bounds, bounds::MemRefCounted, cap::GodotDefault},
    prelude::*,
};

use crate::util::is_fake_clap_error;

/// Set this factory in your root crate to communicate which bespoke cli args type you want to use.
/// Use `assign_bespoke_cli_type::<YourBespokeTypeHere>()`.
pub static ARGS_FACTORY: OnceLock<Box<dyn Send + Sync + Fn() -> Gd<RefCounted>>> = OnceLock::new();

/// This is the Godot-exposed wrapped for the `GAME_ARGS` global.
///
/// If you want to get a CLI arg from `GDScript`, you can't use that one, it's Rust-only.
/// So, instead, you call `ClapCliArgs.args().quality` to get property `quality`.
#[derive(GodotClass)]
#[class(base=Resource, init)]
pub struct ClapCliArgs {
    base: Base<Resource>,
}

#[godot_api]
impl ClapCliArgs {
    #[func]
    /// Get the cli args upcasted to `RefCounted`.
    fn args() -> Gd<RefCounted> {
        ARGS_FACTORY.get().expect("ARGS_FACTORY wasn't setup, please call ARGS_FACTORY.set(...) in your root crate to setup cli args")()
    }
}

#[tracing::instrument(skip_all)]
pub fn parse_cli_godot_args<T: Parser + Debug + Default>() -> T {
    // Make sure you run the game like `game.exe -- --foo`
    // Otherwise it passes --foo to Godot, not your game.

    let cli_args = Os::singleton().get_cmdline_user_args();
    tracing::info!(raw_cli_args = ?cli_args.as_slice());

    let cli_args = cli_args.as_slice().iter().map(ToString::to_string);

    match T::try_parse_from(cli_args) {
        Ok(args) => {
            tracing::info!(parsed_cli_args = ?args);
            args
        }
        Err(err) => {
            if is_fake_clap_error(&err) {
                // Show help if user passed --help
                tracing::info!(%err); // Use % to preserve newlines
            } else {
                tracing::warn!(%err, "failed to parse CLI args");
            }

            // Use default T if invalid
            let default = T::default();
            tracing::info!("using default: {:?}", default);
            default
        }
    }
}

/// Assigns the cli type specific to your game, pass it as generic type argument T.
///
/// `T` must be a `Resource` and must be declared by the user (otherwise we can't call `init()` on it).
pub fn assign_bespoke_cli_type<T>()
where
    T: GodotDefault + Bounds<Memory = MemRefCounted> + Inherits<RefCounted>,
{
    // Note: GodotDefault is NOT the same as Rust's Default trait.
    // It means something we can construct with e.g. T.new() aka T::new_gd()
    // Calling T::new_gd() will call T::init(), defined in your own game's crate.

    // The secret sauce that makes this work: type erasure via upcast()
    match ARGS_FACTORY.set(Box::new(|| T::new_gd().upcast())) {
        Ok(()) => tracing::info!("assigned bespoke CLI type {}", type_name::<T>()),
        Err(_) => tracing::warn!(
            "failed to assign bespoke CLI type {} - did you call assign_bespoke_cli_type() twice?",
            type_name::<T>() // note: this warning also appears when hot-reloading the module in the editor
        ),
    }
}
