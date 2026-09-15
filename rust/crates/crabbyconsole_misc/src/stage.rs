use godot::{classes::Engine, init::InitStage, obj::Singleton as _};
use tracing::info_span;

#[cfg(feature = "tracy")]
use crate::tracy_enabled;
use crate::{
    logging::{drain_log_channels, setup_logging},
    rayon::setup_rayon,
};

/// Call this in your `GDExtension`'s `on_stage_init()`.
///
/// This is called when the library is initialized, for every layer.
/// Use this to setup thread pools, logging, etc.
pub fn on_stage_init(stage: InitStage, init_scene: impl FnOnce()) {
    if stage == InitStage::Core {
        setup_logging(); // Set this up beforehand, outside the match statement, so we can use a span in the next method:
    }

    // Note - putting #[tracing::instrument] on on_stage_init will not work.
    // Why? The subscriber hasn't been setup yet (in setup_logging()).
    // So manually create a span here.
    let span = info_span!("on_stage_init", ?stage);
    let _enter = span.enter();
    tracing::info!("/-- init start --\\");

    if stage == InitStage::Scene {
        init_library(init_scene);

        // Need this cfg here anyway, otherwise we get compilation errors when the feature is disabled.
        #[cfg(feature = "tracy")]
        if tracy_enabled() {
            use crate::tracy::setup_tracy;

            setup_tracy();
        }
    }

    tracing::info!("\\-- init end   --/");
}

/// Call this in your `GDExtension`'s `on_stage_deinit()`.
pub fn on_stage_deinit(stage: InitStage) {
    tracing::info!("--- de-init {stage:?} ---");

    // Note this is called in reverse order:
    match stage {
        InitStage::Core => {
            /* 5 */

            // We need to manually shut down tracy, otherwise the godot process will linger on Windows
            #[cfg(feature = "tracy")]
            if tracy_enabled() {
                tracing::info!(
                    "unloading tracy... if this takes too long, manually stop the session in Tracy"
                );

                // It freezes here if you're still profiling, you have to explicitly stop the session in tracy.

                unsafe {
                    use tracing_tracy::client::sys::___tracy_shutdown_profiler;

                    ___tracy_shutdown_profiler();
                }

                // DO NOT USE tracing after shutting down tracy, use println instead
                // Else it may segfault
                println!("unloaded tracy!");
            }
        }
        InitStage::Servers => { /* 4 */ }
        InitStage::Scene => { /* 3 */ }
        InitStage::Editor => { /* 2 */ }
        InitStage::MainLoop => { /* 1 */ }
        _ => {}
    }
}

#[tracing::instrument(skip_all)]
fn init_library(init_scene: impl FnOnce()) {
    // Enable fancy panic messages in stdout
    match color_eyre::install() {
        Ok(()) => tracing::info!("panic handler initialized"),
        Err(err) => tracing::warn!("failed to install panic handler: {err}"),
    }

    setup_rayon();

    init_scene(); // do your bespoke stuff here

    tracing::info!(
        in_editor = Engine::singleton().is_editor_hint(),
        "initialized"
    );

    // NOTE - check the console to see the godot-rust safeguards level:
    // "Initialize godot-rust (API v4.6.stable.official, runtime v4.6.2.stable.official, safeguards balanced)"
}

/// Call this in your `GDExtension`'s `on_main_loop_frame()`.
pub fn on_main_loop_frame() {
    drain_log_channels();
}
