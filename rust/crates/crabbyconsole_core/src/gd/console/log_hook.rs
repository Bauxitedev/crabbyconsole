//! Logger that hooks into Godot directly and prints to the console.
//!
//! Warning: if `LOG_TO_GODOT` is true, this may recurse infinitely (since it calls `godot_print!`, which then calls your logger, which then calls `godot_print!`, etc)
//! Also, this is Godot 4.5+ so you may need to gate this behind a feature flag so we can support older Godot games.
//!
//! Further reading:
//! - <https://forum.godotengine.org/t/how-to-use-the-new-logger-class-in-godot-4-5/127006>
//! - <https://github.com/godotengine/godot/pull/91006>
//!
//! Also, this requires the `experimental-threads` feature, since `log_message`/`log_error` will be called from other threads.

use std::{
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crabbyconsole_misc::gd::async_node::AsyncGd;
use flume::{Receiver, Sender};
use godot::{
    classes::{ILogger, Logger, ScriptBacktrace, logger::ErrorType},
    obj::Base,
    prelude::*,
};
use regex::Regex;

use crate::gd::console::CrabConsole;

#[derive(GodotClass)]
#[class(no_init,base = Logger)]
pub(super) struct CrabConsoleLogHook {
    base: Base<Logger>,
    hook_tx: Sender<LogHookMsg>,
    enabled: AtomicBool,
}

#[godot_api]
impl ILogger for CrabConsoleLogHook {
    // Warning: This method will be called from threads other than the main thread
    fn log_message(&mut self, message: GString, error: bool) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        // Since this is on another thread, the only way to safely communicate with the console is a flume channel.
        // Note - ensure you do not block here
        let _ = self
            .hook_tx
            .send(LogHookMsg::Message(Arc::new(LogHookMsgMessage {
                message: message.into(),
                error,
            })));
    }

    // Warning: This method will be called from threads other than the main thread
    fn log_error(
        &mut self,
        function: GString,
        file: GString,
        line: i32,
        code: GString,
        rationale: GString,
        _editor_notify: bool,
        error_type: i32,
        _script_backtraces: Array<Gd<ScriptBacktrace>>,
    ) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        // Why does Godot call this "code"? it contains the error message, not just a code
        // Also note you cannot turn a GString into a &str, because Godot strings are UTF-32, while &str must be UTF-8.
        let reason = code.to_string();

        let error_type = ErrorType::from_ord(error_type);
        if is_garbage_error(&reason, error_type) {
            return;
        }

        // Good command to trigger a specific error:
        // 9999999999999999999999999999999999999

        // Function is stored separately, so we don't take it into account when deduplicating the error
        let msg = LogHookMsg::Error(
            function.into(),
            Arc::new(LogHookMsgKeyError {
                file: file.into(),
                line,
                reason,
                rationale: rationale.into(),
                error_type,
            }),
        );

        // Don't log the the error if it was already sent in the last 5 seconds.
        if !deduplicate_error_sync(&msg) {
            return;
        }

        // Since this method is running on another thread, the only way to safely communicate with the console is a flume channel.
        let _ = self.hook_tx.send(msg);
    }
}

#[godot_api]
impl CrabConsoleLogHook {
    /// Check if log hook is enabled.
    #[func]
    pub(super) fn get_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Enable or disable log hook. Returns the new value.
    #[func]
    pub(super) fn set_enabled(&mut self, enabled: bool) -> bool {
        self.enabled.store(enabled, Ordering::Relaxed);
        enabled
    }

    /// Toggle log hook. Returns the new value.
    #[func]
    pub(super) fn toggle_enabled(&mut self) -> bool {
        let old_val = self.enabled.fetch_xor(true, Ordering::Relaxed);
        !old_val
    }

    pub(super) fn new(enabled: bool, hook_tx: Sender<LogHookMsg>) -> Gd<Self> {
        Gd::from_init_fn(|base| Self {
            enabled: enabled.into(),
            hook_tx,
            base,
        })
    }
}

/// We use Arc excessively here since we have a lot of fields and we don't want to clone them.
/// Function is stored separately, so we don't take it into account when deduplicating the error
pub(super) enum LogHookMsg {
    Message(Arc<LogHookMsgMessage>),
    Error(String, Arc<LogHookMsgKeyError>), // String = function
}

/// This is the type that gets used as key in the hashmap.
/// Notice how it doesn't store the function name, so it gets ignored in the uniqueness check.
#[derive(Hash, PartialEq, Eq)]
pub(super) enum LogHookMsgKey {
    Message(Arc<LogHookMsgMessage>),
    Error(Arc<LogHookMsgKeyError>),
}

#[derive(Hash, PartialEq, Eq)]
pub(super) struct LogHookMsgMessage {
    message: String,
    error: bool,
}

#[derive(Hash, PartialEq, Eq)]
pub(super) struct LogHookMsgKeyError {
    // no function
    file: String,
    line: i32,
    reason: String,
    rationale: String,
    error_type: godot::classes::logger::ErrorType,
}

impl From<&LogHookMsg> for LogHookMsgKey {
    fn from(value: &LogHookMsg) -> Self {
        match value {
            LogHookMsg::Message(msg) => Self::Message(Arc::clone(msg)),
            LogHookMsg::Error(_function, err) => Self::Error(Arc::clone(err)),
        }
    }
}

impl CrabConsole {
    #[tracing::instrument(skip_all)]
    pub(super) async fn log_hook_task(mut self: AsyncGd<Self>, hook_rx: Receiver<LogHookMsg>) {
        tracing::info!("log hook task starting...");

        let mut rich_text = self.bind_mut().nodes.console_history.clone();

        while let Ok(msg) = hook_rx.recv_async().await {
            // Warning - do not use tracing::___ here if LOG_TO_GODOT is true, or you get an infinite loop.

            self.bind_mut().maybe_add_timestamp();

            match msg {
                LogHookMsg::Message(inner) => {
                    let LogHookMsgMessage { message, error } = inner.as_ref();
                    // Do not use append_text, it parses bbcode in the message.

                    if *error {
                        rich_text.push_color(Color::RED);
                    } else {
                        rich_text.push_color(Color::DARK_GRAY);
                    }

                    // TODO make a method that takes a string with ANSI color codes and puts them in rich_text::push_color/pop?
                    rich_text.add_text(message);

                    rich_text.pop(); //pop color
                }
                LogHookMsg::Error(function, inner) => {
                    let LogHookMsgKeyError {
                        file,
                        line,
                        reason,
                        rationale,
                        error_type,
                        ..
                    } = inner.as_ref();
                    // Do not use append_text, it parses bbcode in the message, may be risky.

                    rich_text.push_color(Color::RED);
                    rich_text.push_bold();
                    rich_text.add_text(&format!("Error ({error_type:?}): "));
                    rich_text.pop(); //pop bold
                    rich_text.pop(); //pop color

                    rich_text.push_color(Color::ORANGE);
                    rich_text.add_text(&format!("{reason} ({rationale})\n"));
                    rich_text.pop(); //pop color

                    // TODO may need another call to this.bind_mut().maybe_add_timestamp() here

                    rich_text.push_color(Color::RED);
                    rich_text.push_bold();
                    rich_text.add_text("Function: ");
                    rich_text.pop(); //pop bold
                    rich_text.pop(); //pop color

                    rich_text.push_color(Color::ORANGE);
                    rich_text.add_text(&format!("{function} ("));
                    rich_text.push_color(Color::GRAY);
                    rich_text.add_text(&format!("{file}:{line}"));
                    rich_text.pop(); //pop color
                    rich_text.add_text(")\n");
                    rich_text.pop(); //pop color
                }
            }
        }
        tracing::warn!("log hook task shutdown");
    }
}

fn is_garbage_error(reason: &str, error_type: ErrorType) -> bool {
    if error_type == ErrorType::SCRIPT {
        // We filter out this error, otherwise if you type print("foo") in the console it spams errors.
        // Note - this matches the occurrence of the error anywhere in `reason`, which is good.
        // `reason` starts with "Parse error:" it seems, so the actual error comes later.
        static RE_ERROR_RETURN_VALUE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"Cannot get return value of call to ".+\(\)" because it returns "void"\."#)
                .unwrap()
        });

        // This one is only triggered by get_node("/root/PickerTestScene/Anim Sprite With Space").play() for some reason
        // Note: technically doesn't need to be a regex, but eh, should be fast enough.
        static RE_ERROR_RETURN_VALUE2: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"Trying to get a return value of a method that returns "void""#).unwrap()
        });

        // We filter out this error as well, otherwise it spams errors every time you assign a variable.
        // Note: technically doesn't need to be a regex, but eh, should be fast enough.
        static RE_ERROR_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"Assignment is not allowed inside an expression."#).unwrap()
        });

        // This one too, otherwise it spams errors if you do e.g. for c in "foo": print(c)
        static RE_ERROR_END_OF_STATEMENT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"Expected end of statement after .+, found ".+" instead\."#).unwrap()
        });

        // In the future if we have even more regexes we can make a RegexSet
        if RE_ERROR_RETURN_VALUE.is_match(reason)
            || RE_ERROR_RETURN_VALUE2.is_match(reason)
            || RE_ERROR_ASSIGNMENT.is_match(reason)
            || RE_ERROR_END_OF_STATEMENT.is_match(reason)
        {
            return true;
        }
    }

    false
}

/// Copy pasted from ui.rs except this time it has to be thread-safe, so can't use BoxedCache.
fn deduplicate_error_sync(err: &LogHookMsg) -> bool {
    // We don't need RefCell since this is a Sync Cache.
    // So, all methods on Cache are &self instead of &mut self.
    // Note - technically we don't need the Arc<> around the Cache, but keeping it anyway.
    // Don't want the TLS buffer filling up.
    thread_local! {
        static HOOK_ERROR_DEDUPLICATOR: Arc<mini_moka::sync::Cache<LogHookMsgKey, ()>> = {
            Arc::new(
                mini_moka::sync::Cache::builder()
                    .time_to_live(Duration::from_secs(5)) // Use TTL instead of TTI, otherwise it will live too long
                    .build(),
            )
        };
    }

    let key = LogHookMsgKey::from(err); // convert &LogHookMsg -> LogHookMsgKey
    HOOK_ERROR_DEDUPLICATOR.with(|c| {
        if c.contains_key(&key) {
            return false;
        }

        c.insert(key, ());
        true
    })
}
