//! This module sets up logging/tracing.
//!
//! Note - there is no point in doing `#[cfg_attr(feature = "enable-tracing", instrument)]` instead of `#[instrument]`
//! so we can disable tracing by disabling the `enable-tracing` feature, for performance reasons.
//!
//! Since tracing has the option to compile away certain levels at compile time.
//! See <https://docs.rs/tracing/latest/tracing/level_filters/index.html#compile-time-filters>/
//!
//! Note - this is the latest version of the logging module, Let's call it v1 (28-05-2026)

use std::{
    borrow::Cow,
    io::{self, Write},
    sync::{LazyLock, OnceLock},
};

use flume::{Receiver, Sender};
use godot::{
    classes::{ProjectSettings, class_macros::private::virtuals::Os::Variant},
    global::{godot_print, printraw},
    obj::Singleton as _,
};
use time::macros::format_description;
use tracing::level_filters::LevelFilter;
use tracing_error::ErrorLayer;
use tracing_subscriber::{
    EnvFilter, Registry,
    fmt::{MakeWriter, time::LocalTime},
    layer::SubscriberExt as _,
    reload,
    util::SubscriberInitExt as _,
};

use crate::flags::LOG_TO_GODOT_FLAG;
#[cfg(feature = "tracy")]
use crate::tracy_enabled;

type FilterHandle = reload::Handle<EnvFilter, Registry>;
static FILTER_HANDLE: OnceLock<FilterHandle> = OnceLock::new();

/// Call this from anywhere to change tracing-subscriber's log level in real time
pub fn set_log_level(filter: &str) {
    if let Some(handle) = FILTER_HANDLE.get() {
        // TODO EnvFilter::new(filter) ignores all invalid directives! Maybe print warning if it's invalid
        let new_env_filter = EnvFilter::new(filter);
        dbg!(&new_env_filter.to_string());
        handle.reload(new_env_filter).unwrap();
    }
}

pub fn get_log_level() -> Option<String> {
    // I'm not sure if this can ever be None, but we gotta handle that case anyway
    if let Some(handle) = FILTER_HANDLE.get() {
        handle
            .clone_current()
            .map(|envfilter| envfilter.to_string())
    } else {
        None
    }
}

// We can't use [tracing::instrument] here methinks
pub fn setup_logging() {
    let timer = LocalTime::new(format_description!(
        "[hour]:[minute]:[second].[subsecond digits:3]"
    ));

    dbg!(LOG_TO_GODOT_FLAG.get());

    // Logging to godot enables using godot's log-to-disk functionality, but it's probably slower than stdout
    let writer = move || -> Box<dyn io::Write> {
        // This closure gets called for every event, so we can change it in real time!
        if LOG_TO_GODOT_FLAG.get() {
            Box::new(GodotWriter {}) // GodotWriter is a ZST, so this Box doesn't allocate at all
        } else {
            Box::new(io::stdout())
        }
    };

    let final_filter = load_env_filter();
    let (reload_filter, handle) = reload::Layer::new(final_filter); // add EnvFilter reload functionality
    FILTER_HANDLE
        .set(handle)
        .unwrap_or_else(|_| println!("Warning - FILTER_HANDLE was already initialized"));

    let layer = tracing_subscriber::fmt::layer()
        .with_timer(timer)
        .with_writer(writer);
    // Use with_span_events() and FmtSpan::CLOSE to print span duration
    // Or use .with_thread_ids(true) to print thread ids

    let result = {
        let base = tracing_subscriber::registry()
            .with(reload_filter)
            .with(layer)
            .with(ErrorLayer::default());

        // Only add the tracy layer if 1. the `tracy` feature is enabled and 2. we're not in the editor
        #[cfg(feature = "tracy")]
        let tracy_layer = if tracy_enabled() {
            println!("Using tracy layer!");
            Some(tracing_tracy::TracyLayer::new(
                tracy::GodotTracyLayerConfig::default(),
            ))
        } else {
            println!("NOT using tracy layer!");

            None
        };

        #[cfg(feature = "tracy")]
        let base = base.with(tracy_layer); // This works because https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Layer.html#impl-Layer%3CS%3E-for-Option%3CL%3E

        base.try_init()
    };

    match result {
        Ok(()) => {
            tracing::info!("tracing enabled");
        }
        Err(err) => {
            // This happens when the Godot editor reloads this library
            // -> "a global default trace dispatcher has already been set"
            tracing::warn!("failed to init tracing_subscriber: {err}");
        }
    }
}

#[cfg(feature = "tracy")]
mod tracy {

    use super::*;

    #[derive(Default)]
    pub(super) struct GodotTracyLayerConfig {
        fmt: DefaultFields,
    }
    impl tracing_tracy::Config for GodotTracyLayerConfig {
        type Formatter = DefaultFields;
        fn formatter(&self) -> &Self::Formatter {
            &self.fmt
        }
        // The boilerplate ends here

        /// Collect 64 frames in stack traces for entering spans.
        /// Enable this if you want a callstack per message.
        /// However, "Note that enabling callstack collection can and will introduce a non-trivial overhead at
        /// every instrumentation point"
        /// Actually let's use 60, since Tracy limits to 62
        fn stack_depth(&self, _: &tracing::Metadata) -> u16 {
            60
            //0 = default (no call stack)
        }

        // Overrode this otherwise we get ugly zone names with ANSI color characters inside of them
        fn format_fields_in_zone_name(&self) -> bool {
            false
        }
    }
}

pub fn load_env_filter() -> EnvFilter {
    // NOTE - don't call eyre! in here, or ErrorLayer will panic later

    let settings = ProjectSettings::singleton();
    let godot_rust_log_key = "crabbyconsole/logging/default_rust_log";

    let default_filter = LevelFilter::ERROR; // Unused if default_rust_log is set in your project settings
    let envvar_filter = EnvFilter::builder().try_from_env();
    let godot_filter = settings
        .has_setting(godot_rust_log_key)
        .then(|| {
            settings
                .get_setting_with_override(godot_rust_log_key) // Important - read the config override for exported games
                .to_string()
        })
        .ok_or("setting missing".to_owned())
        .and_then(|val| {
            if val.is_empty() {
                return Err("empty string".to_owned());
            }

            println!("Value of {godot_rust_log_key} = {val}");
            EnvFilter::builder()
                .parse(val) //empty string is not valid
                .map_err(|err| err.to_string())
        });

    if let Ok(filt) = envvar_filter {
        println!("Using environment filter: {filt}");
        filt
    } else if let Ok(filt) = godot_filter {
        println!("Using {godot_rust_log_key} filter: {filt}");
        filt
    } else {
        println!("Using default filter: {default_filter}");

        EnvFilter::new(default_filter.to_string())
    }
}
pub struct GodotWriter;

impl Write for GodotWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let cow = String::from_utf8_lossy(buf); // Convert raw bytes to string
        let s = remove_trailing_newline(cow);

        // godot_print! internally calls print()
        // See https://docs.rs/godot-core/0.4.5/src/godot_core/global/print.rs.html#117

        // Note - printraw prints to the terminal, NOT the Godot editor.
        // However, the output DOES seem to become part of the log files, which is what we want.
        // The only thing missing are the panic messages from color-eyre

        // TODO since Godot 0.5.x it seems we have new methods to work with:
        // - https://godot-rust.github.io/docs/gdext/master/godot/global/fn.print_custom.html
        // - https://godot-rust.github.io/docs/gdext/master/godot/classes/trait.ILogger.html

        // potentially faster version:
        // let s = str::from_utf8(buf)
        //     .map(|s| Cow::Borrowed(s))
        //     .unwrap_or_else(|_| String::from_utf8_lossy(buf));
        log_to_channel(s.into_owned()); //  note - into_owned clones if it's a Cow::Borrowed

        Ok(buf.len()) // note - do NOT return s.len() here, instead return buf.len() (this is the amount of INPUT bytes written, not OUTPUT bytes written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for GodotWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        Self
    }
}

fn remove_trailing_newline(input: Cow<str>) -> Cow<str> {
    match input {
        Cow::Owned(owned) => {
            if let Some(stripped) = owned.strip_suffix('\n') {
                Cow::Owned(stripped.to_owned())
            } else {
                Cow::Owned(owned)
            }
        }
        Cow::Borrowed(borrowed) => {
            if let Some(stripped) = borrowed.strip_suffix('\n') {
                Cow::Borrowed(stripped)
            } else {
                Cow::Borrowed(borrowed)
            }
        }
    }
}

/// Formats a `Gd<T>`, by taking its instance ID as hexadecimal
#[macro_export]
macro_rules! format_gdobj {
    ($this:expr) => {
        format_args!("{:#x}", $this.instance_id().to_i64())
    };
}

pub fn format_as_pointer<T>(val: &T) -> String {
    format!("{:#x}", val as *const T as usize)
}
static LOG_CHANNELS: LazyLock<(Sender<String>, Receiver<String>)> = LazyLock::new(flume::unbounded);

/// Empty `LOG_RX` to log all pending messages on the main thread.
#[tracing::instrument(skip_all)]
pub fn drain_log_channels() {
    while let Ok(msg) = LOG_CHANNELS.1.try_recv() {
        let use_printraw = false; // TODO add a flag in flags.rs for this?

        if use_printraw {
            printraw(&[Variant::from(msg)]);
            // This conversion step looks slow to me...
            // However, that's exactly what godot_print does:
            /*
                macro_rules! godot_print {
                    ($fmt:literal $(, $args:expr_2021)* $(,)?) => {
                        $crate::global::print(&[
                            $crate::builtin::Variant::from(
                                format!($fmt $(, $args)*)
                            )
                        ])
                    };
                }
            */
        } else {
            godot_print!("{}", msg); // TODO profile this. If slow, rate limit it. 
            // Update - yep, seems slow! drain_log_channels appears in the profiler.
            // Rate limiting would be a decent idea here
        }
    }
}

fn log_to_channel(msg: String) {
    let _ = LOG_CHANNELS.0.send(msg);
}
