use crabbyconsole_clap::cli::CrabConsoleArgs;
use crabbyconsole_core::gd::console::util::{BuildInfo, assign_build_info};
use crabbyconsole_misc::{
    gd::cli::assign_bespoke_cli_type,
    stage::{on_main_loop_frame, on_stage_deinit, on_stage_init},
};
use godot::prelude::*;

// Usage of other dependencies must be explicitly declared; otherwise, they won't be registered.
// See https://godot-rust.github.io/docs/gdext/master/godot/init/trait.ExtensionLibrary.html#using-other-gdextension-libraries-as-dependencies
extern crate crabbyconsole_core;
extern crate crabbyconsole_misc;
extern crate crabbyconsole_test_registry; // <-- bring in the CrabConsoleTestRunner

struct CrabConsoleExtension;

#[gdextension]
unsafe impl ExtensionLibrary for CrabConsoleExtension {
    // Note - do not tracing::instrument this method or it may mess with tracy
    fn on_stage_init(stage: InitStage) {
        on_stage_init(stage, || {
            // Print crate details
            // TODO env!("VERGEN_BUILD_TIMESTAMP") doesn't work properly! It only updates when you modify the build.rs file!
            tracing::info!(
                crate = %env!("CARGO_PKG_NAME"), // This must be in `root` crate or this will be wrong
                version = %env!("CARGO_PKG_VERSION"),
                build_id = %env!("BUILD_ID"),
                build_profile = %env!("ACTUAL_PROFILE"),
                build_features = %env!("BUILD_FEATURES"), // Cargo features, NOT godot features
                //  env!("VERGEN_BUILD_TIMESTAMP")
            );

            assign_bespoke_cli_type::<CrabConsoleArgs>();
            assign_build_info(BuildInfo {
                build_id: env!("BUILD_ID"),
                actual_profile: env!("ACTUAL_PROFILE"),
                build_features: env!("BUILD_FEATURES"),
            });

            // TODO add console custom commands/plugins/flags here?
            // you can inject a plugin/closure into CrabConsole here that gets a flag from the flags crate?
            // so console doesn't have to depend on flags at all
        });
    }

    //#[tracing::instrument()] // let's not instrument this one since it may mess with tracy's shutdown logic
    fn on_stage_deinit(stage: InitStage) {
        on_stage_deinit(stage);
    }

    fn on_main_loop_frame() {
        on_main_loop_frame();
    }
}
