use std::{
    cell::LazyCell,
    sync::Arc,
    time::{Duration, Instant},
};

use color_eyre::eyre::{Report, eyre};
use crabbyconsole_clap::plot::PlotAction;
use crabbyconsole_misc::{
    FutureTracyExt as _,
    gd::{
        async_node::{AsyncGd, AsyncNode, TaskGroups},
        autoload::tracy_manager::TracyManager,
    },
    reflection::{ConsoleDebug, OpaqueDebug},
    util::get_viewport,
};
use flume::Sender;
use futures::executor::block_on;
use godot::{
    classes::{
        CanvasLayer, GDScript, HSeparator, InputEvent, InputEventKey, LineEdit, Logger, MenuButton,
        Os, RichTextLabel, Texture2D, VSplitContainer,
    },
    global::{Key, push_error},
    prelude::*,
};
use indexmap::IndexMap;
use mini_moka::unsync::Cache as UnsyncCache;
use rand::RngExt;
use tap::Tap as _;

use crate::gd::console::{
    r#async::USE_EVAL_TIMEOUT,
    autocomplete::{SuggestionSlots, Suggestions},
    eval::{
        context::CrabConsoleScriptContext,
        custom::{CommandName, CustomCommand},
        draw::CrabConsoleDebugDrawer,
        plot::PlotTextureCacheSlot,
        singletons::get_all_singletons,
        vr::{VrState, enter_vr},
        watch::Watches,
    },
    history::HistoryEntry,
    job::{Job, JobExpression, JobExpressionInner, JobInner},
    lockdown::enable_lockdown,
    log_hook::CrabConsoleLogHook,
    remote::ConsoleServer,
};

pub mod r#async;
pub mod autocomplete;
pub mod eval;
pub mod history;
pub mod input_map;
pub mod job;
pub mod lockdown;
pub mod log_hook;
pub mod picker;
pub mod remote;
pub mod ui;
pub mod util;

#[derive(Debug)]
struct CrabConsoleNodes {
    // No longer exporting these, since accessing them in the console causes a double-borrow panic.
    canvas_layer: Gd<CanvasLayer>,
    vsplitter: Gd<VSplitContainer>,
    line_edit: Gd<LineEdit>,
    console_history: Gd<RichTextLabel>,

    watch_label: Gd<RichTextLabel>,
    remote_label: Gd<RichTextLabel>,
    autocomplete_label: Gd<RichTextLabel>,

    busy_indicator: Gd<HSeparator>,
    ellipsis_button: Gd<MenuButton>,

    debug_drawer: Gd<CrabConsoleDebugDrawer>,
    picker: Gd<Node2D>,
}

/// This is better than the Godot debugger evaluator, since it only works if you're stuck on a breakpoint.
/// See <https://github.com/godotengine/godot/issues/99005>.
/// Otherwise it prints a vague error.
#[derive(GodotClass, Debug)]
#[class(base=Node)]
pub struct CrabConsole {
    base: Base<Node>,

    #[export]
    logo: OnEditor<Gd<Texture2D>>,

    nodes: OnReady<CrabConsoleNodes>,
    initial_rich_text: OnReady<String>, // The initial BBCoded text in the RichTextLabel

    script_context: OnReady<Gd<CrabConsoleScriptContext>>,
    executors: TaskGroups<Self>,
    master_script: OnReady<Gd<GDScript>>,

    // This should NOT be a global thread_local!, or it will panic when you close the game
    // (since the PackedStringArray/Variants are dropped after Godot is already closed, so godot-rust can't request Godot to clean them up anymore).
    singletons: OpaqueDebug<LazyCell<(PackedStringArray, Array<Variant>)>>,

    job_rx: OnReady<flume::Receiver<Job>>,

    // do not store this in a thread_local, or it will panic when you close the game (since Gd's destructor runs too late)
    job_result_cache: OpaqueDebug<UnsyncCache<InstanceId, Gd<Texture2D>>>,
    plot_texture_cache: OpaqueDebug<UnsyncCache<PlotAction, PlotTextureCacheSlot>>,

    bindings: IndexMap<Key, String>,
    watches: Watches,
    commands: IndexMap<CommandName, CustomCommand>, // Custom commands (using newtype to ensure the name is always valid)
    search_mode_enabled: bool,                      // <-- Add to ConsoleConfig?
    timestamps_enabled: bool,                       // <-- Add to ConsoleConfig?
    log_hook: Option<Gd<CrabConsoleLogHook>>, // Can't use OnReady here, since we init in _enter_tree(), not _ready()

    history: OpaqueDebug<Vec<HistoryEntry>>,
    history_index: usize,
    history_chan: (flume::Sender<HistoryEntry>, flume::Receiver<HistoryEntry>),
    autocomplete_chan: (flume::Sender<String>, flume::Receiver<String>),
    suggestions: Suggestions,

    server: Option<ConsoleServer>, // None = the remote console is not running
    timeout: f64, // Max timeout before async/signal command in console is killed (in seconds)  // <-- Add to ConsoleConfig?

    last_frame_time: OnReady<(Instant, f64)>,
    last_watch_label_update: Instant,

    vr_state: Option<VrState>,
}

#[godot_api]
impl INode for CrabConsole {
    fn init(base: Base<Node>) -> Self {
        // Note - this is also called in the editor
        // Need type hint here for some reason
        let singletons: LazyCell<_> = LazyCell::new(|| {
            let mut input_names = PackedStringArray::new();
            let mut input_values: Array<Variant> = Array::new();

            for (name, value) in get_all_singletons() {
                input_names.push(name);
                input_values.push(&value);
            }

            assert_eq!(input_names.len(), input_values.len());

            (input_names, input_values)
        });

        let nodes = OnReady::from_base_fn(|base| CrabConsoleNodes {
            canvas_layer: base.get_node_as("%ConsoleCanvasLayer"),
            vsplitter: base.get_node_as("%ConsoleVSplit"),
            line_edit: base.get_node_as("%ConsoleLineEdit"),
            console_history: base.get_node_as("%ConsoleHistory"),

            watch_label: base.get_node_as("%WatchLabel"),
            remote_label: base.get_node_as("%RemoteConsoleLabel"),
            autocomplete_label: base.get_node_as("%AutocompleteLabel"),

            busy_indicator: base.get_node_as("%BusyIndicator"),
            ellipsis_button: base.get_node_as("%EllipsisButton"),

            debug_drawer: base.get_node_as("%DebugDrawer"),
            picker: base.get_node_as("%ConsolePicker"),
        });

        Self {
            base,

            logo: OnEditor::default(), // expects user to set value in editor

            script_context: OnReady::manual(),
            executors: Default::default(),
            job_rx: OnReady::manual(),
            job_result_cache: OpaqueDebug(
                UnsyncCache::builder()
                    .time_to_idle(Duration::from_secs(1)) // use TTI here, not TTL
                    .build(), //                      ^ keep this very short to avoid filing up VRAM
            ),
            plot_texture_cache: OpaqueDebug(
                UnsyncCache::builder()
                    .time_to_idle(Duration::from_mins(10)) // use TTI here, not TTL
                    .build(), //                      ^^ should be longer than the max duration of a :watch pause, e.g. 10 min or longer
            ),

            history: OpaqueDebug(vec![]),
            history_index: usize::MAX,
            history_chan: flume::unbounded(),
            bindings: IndexMap::default(),
            watches: Watches::default(),
            commands: IndexMap::default(),

            nodes,
            initial_rich_text: OnReady::manual(),

            server: None,
            timeout: 20.,

            autocomplete_chan: flume::unbounded(),
            singletons: OpaqueDebug(singletons),
            suggestions: Suggestions {
                slots: SuggestionSlots::None,
                dirty: true, // <-- use false if you want there to be no autocompletion slots at the start
            },

            search_mode_enabled: false,
            timestamps_enabled: false,
            log_hook: None,
            last_frame_time: OnReady::manual(),
            last_watch_label_update: Instant::now() - Duration::from_hours(1), // Hacky way to make the label update immediately

            master_script: OnReady::from_base_fn(|_| {
                let mut script = GDScript::new_gd();

                // Let's use a short path here, may reduce mem leak -> didn't work?
                let id: u16 = rand::rng().random();
                let script_path = format!("res://cc_{id}.gd");
                tracing::info!("taking over path `{script_path}`...");
                script.take_over_path(&script_path);
                script
            }),
            vr_state: None,
        }
    }

    #[tracing::instrument(skip_all)]
    fn ready(&mut self) {
        // Create the CrabConsoleScriptContext and point it to ourselves
        self.script_context
            .init(CrabConsoleScriptContext::new(self.to_gd()));

        self.nodes.canvas_layer.hide(); // <-- it's visible in the editor, but hidden when you start the game

        // This is needed otherwise autocomplete_label will not resize to zero
        self.nodes.autocomplete_label.set_text("");
        self.set_busy_indication(false);
        self.initial_rich_text
            .init(self.nodes.console_history.get_text().to_string()); // includes BBCode

        let (job_tx, job_rx) = flume::unbounded();
        self.job_rx.init(job_rx);
        self.start_all_tasks(job_tx);
        Self::setup_input_maps();

        // Add the TracyManager one frame later, or it fails
        self.base()
            .linked_callable("add_tracy_manager", |_| {
                TracyManager::add_autoload_if_missing();
            })
            .call_deferred(&[]);

        // Connect line_edit gui_event to detect up/down/tab events
        // Connect line_edit text_changed to detect text change events
        {
            let line_edit = self.nodes.line_edit.clone();

            line_edit.signals().gui_input().connect_other(self, {
                let line_edit = line_edit.clone();
                move |this, e| {
                    let line_edit = line_edit.clone();
                    this.on_line_edit_gui_event(line_edit, e)
                }
            });

            let line_edit = self.nodes.line_edit.clone();
            line_edit.signals().text_changed().connect_other(self, {
                let line_edit = line_edit.clone();
                move |this, txt| {
                    let line_edit = line_edit.clone();
                    this.on_line_edit_text_changed(line_edit, txt.to_string())
                }
            });
        }

        self.last_frame_time.init((Instant::now(), 0.0));
    }

    #[tracing::instrument(skip_all)] // <-- TODO are you sure you want to instrument this? it's called every frame
    fn process(&mut self, _delta: f64) {
        // Update frame time
        *self.last_frame_time = {
            // Note - frame_time != delta (in slow motion, or due to Godot applying delta smoothing)
            let now = Instant::now();

            // Do not use elapsed(), it will be slightly off (since you'd call Instant::now() twice)
            let frame_time = (now - self.last_frame_time.0).as_secs_f64();
            (now, frame_time)
        };

        //////

        self.tick_deferred();

        // Note - should we tick after these? Since they both spawn async jobs?
        self.run_pending_console_jobs();
        self.run_pending_watch_jobs();

        // Rate-limit update_watch_text() - important on high framerates (120+ FPS) to avoid introducing CPU bottleneck
        let watch_label_update_rate = 15.; // How many times per second to update the label
        if self.last_watch_label_update.elapsed().as_secs_f32() > 1.0 / watch_label_update_rate {
            self.update_watch_text();
            self.last_watch_label_update = Instant::now(); // or re-use now() instead of elapsed()?
        }

        // This can be done every frame, since it's tiny
        self.update_remote_text();
    }

    fn input(&mut self, event: Gd<InputEvent>) {
        // Note - this is `handled` input, so this will always work, even when the textbox is focused.

        if event.is_action_pressed("crabbyconsole_toggle") {
            get_viewport().set_input_as_handled(); // This line is important - it prevents ` from appearing in textbox
            self.signals().console_toggle_requested().emit(); // TODO maybe emit "true" or "false" depending on current visibility?
        }
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        // Handle custom bindings
        if let Ok(e) = event.try_cast::<InputEventKey>()
            && e.is_pressed()
            && let Some(expression) = self.bindings.get(&e.get_keycode()).cloned()
        {
            self.bound_task()
                .new(async move |this| {
                    let _ = this
                        .clone()
                        .eval_job_without_channel(JobExpressionInner::String(Arc::from(expression)))
                        .with_tracy_non_continuous_frame("eval_bind_job")
                        .await; // ignore error
                })
                .spawn();
        }
    }

    /// Add the logger in `enter_tree()` and remove the logger in `exit_tree()`.
    ///
    /// Note: official docs recommend adding the logger in `_init`, but then it won't be cleared up
    /// when you remove the `CrabConsole` from the tree.
    /// See <https://docs.godotengine.org/en/stable/tutorials/scripting/logging.html#creating-custom-loggers>
    #[tracing::instrument(skip_all)]
    fn enter_tree(&mut self) {
        let (hook_tx, hook_rx) = flume::unbounded();
        let log_hook = CrabConsoleLogHook::new(true, hook_tx); // pass a flume channel into CrabConsoleLogHook, so it can send messages to us.
        Os::singleton().add_logger(&log_hook.clone().upcast::<Logger>());

        self.log_hook = Some(log_hook);

        // Wait for log messages to come in
        self.bound_task()
            .new(async |this| this.log_hook_task(hook_rx).await)
            .spawn();

        tracing::info!("added log hook");
    }

    #[tracing::instrument(skip_all)]
    fn exit_tree(&mut self) {
        if let Some(log_hook) = self.log_hook.take() {
            Os::singleton().remove_logger(&log_hook.upcast::<Logger>());
            tracing::info!("removed log hook");

            // at this point the two references to CrabConsoleLogHook should be dropped.
            // (1 = self.log_hook, 2 = the reference Godot holds after calling add_logger())
            // so, hook_tx should be dropped, and the async loop above should terminate.
        }
    }
}

#[godot_api]
impl CrabConsole {
    /// Fired when user requests to toggle the console's visibility.
    #[signal]
    pub fn console_toggle_requested();

    /// Fired when user requests to move up/down in history, usually by pressing up or down (-1 = up, 1 = down).
    #[signal]
    pub fn history_move_requested(direction: i32);

    /// Fired when the user requests to perform an autocompletion, usually by pressing Tab or Shift+Tab.
    #[signal]
    pub fn autocomplete_perform_requested(direction: i64);

    /// Fired when the user requests to toggle history search mode, usually by pressing Ctrl+R.
    #[signal]
    pub fn history_search_toggle_requested();
    /// Fired when the user requests to interrupt the currently running command, usually by pressing Ctrl+C.
    #[signal]
    pub fn interrupt_requested();

    /// Fired when user requests to start the remote console on the given host/port.
    #[signal]
    pub fn remote_console_start_requested(host: GString, port: u16); // Needs to be a `GString`, otherwise we can't call `into_future()` on it - all arguments must impl the trait `IntoDynamicSend`.

    /// Fired when user requests to stop the remote console.
    #[signal]
    pub fn remote_console_stop_requested();

    /// Fired when remote console started successfully. Note: the host/port can be different than what the user requested (e.g. if user requests port 0, it will assign a random port)
    #[signal]
    pub fn remote_console_started(host: GString, port: u16);

    /// Fired when remote console start failed (e.g. port already bound, or no permission to bind on a port < 1023)
    #[signal]
    pub fn remote_console_start_failed(error: GString);

    #[func]
    fn request_remote_console_start(&mut self, host: String, port: u16) {
        // Call this from gdscript
        self.signals()
            .remote_console_start_requested()
            .emit(&host, port);
    }

    /// Starts all async tasks needed to make the CrabConsole work
    fn start_all_tasks(&mut self, job_tx: Sender<Job>) {
        // Having separate tasks here is useful in case one of them panics.
        // Then only that specific task will stop, it will not affect the others.

        let job_tx2 = job_tx.clone();
        let task = self.bound_task();

        // Basic tasks
        task.new(Self::toggle_visibility_task).spawn();
        task.new(Self::toggle_autocomplete_visibility_task).spawn();
        task.new(Self::line_edit_task).spawn();
        task.new(async |this| this.line_eval_task(job_tx).await)
            .spawn();
        task.new(async |this| this.remote_console_task(job_tx2).await)
            .spawn();
        task.new(Self::autocomplete_task).spawn();

        // History stuff
        task.new(async |this| this.save_history_task().await)
            .spawn();
        task.new(async |this| this.load_history_task().await)
            .spawn();

        // Node picker task
        task.new(async |this| this.picker_task().await).spawn();
    }

    #[tracing::instrument(skip_all)]
    fn on_line_edit_gui_event(&mut self, line_edit: Gd<LineEdit>, event: Gd<InputEvent>) {
        if line_edit.is_editing() {
            let event = EventChecker {
                event: &event,
                line_edit: line_edit.clone(),
            };

            // Important: use `else if` to prevent triggering two branches at the same time
            if event.is("ui_down", true) {
                self.signals().history_move_requested().emit(1);
            } else if event.is("ui_up", true) {
                self.signals().history_move_requested().emit(-1);
            } else if event.is("ui_focus_next", true) {
                self.signals().autocomplete_perform_requested().emit(1);
            } else if event.is("ui_focus_prev", true) {
                self.signals().autocomplete_perform_requested().emit(-1);
            } else if event.is("crabbyconsole_search_toggle", false) {
                self.signals().history_search_toggle_requested().emit();
            } else if event.is_with_cond(
                "crabbyconsole_interrupt",
                false,
                !line_edit.has_selection(),
            ) {
                // Only accept the event and emit the signal if we have NO selection (else, Ctrl+C should just copy the text).
                self.signals().interrupt_requested().emit();
            }
        }
    }

    #[tracing::instrument(skip_all)]
    fn on_line_edit_text_changed(&mut self, _line_edit: Gd<LineEdit>, txt: String) {
        // we send on channel here, NOT a godot signal
        // (since we need to retain the messages across multiple frames, in case autocomplete is slow)
        let _ = self.autocomplete_chan.0.send(txt);
    }

    #[tracing::instrument(skip_all)]
    fn run_pending_console_jobs(&mut self) {
        let task = self.bound_task();

        let job_rx = self.job_rx.clone();
        for job in job_rx.try_iter() {
            task.new(async |this| {
                let _ = this
                    .eval_job(JobInner::from(job))
                    .with_tracy_non_continuous_frame("eval_job")
                    .await;
            })
            .spawn();
        }
    }

    /// Evaluates the given command (if async or a signal, blocks until it completed)
    /// Call this in your game to configure the console if needed.
    /// Also useful when running commands in the console that use for loops.
    /// If the command fails, returns nil and prints the error.
    ///
    /// This is called "blocking" to indicate this may block the main thread, so it may freeze your game. Be careful!
    ///
    /// Note - `eval_blocking` ALWAYS uses a timeout, to prevent the game locking up forever if you run a never ending future
    #[func(gd_self)]
    pub fn eval_blocking(this: Gd<Self>, expression: String) -> Variant {
        let expression = Arc::from(expression);
        let result: impl FnOnce() -> Result<_, Report> = || {
            this.bind().ensure_ready("eval_blocking")?;

            Ok(block_on(USE_EVAL_TIMEOUT.scope(true, async {
                AsyncGd(this)
                    .eval_job_without_channel(JobExpressionInner::String(Arc::clone(&expression)))
                    .with_tracy_non_continuous_frame("eval_blocking")
                    .await
            }))?)
        };

        match result() {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(%err, %expression, "eval_blocking failed to evaluate expression");
                push_error(&[Variant::from(format!(
                    "eval_blocking failed to evaluate expression: {err}"
                ))]);

                Variant::nil()
            }
        }

        // Later we can make a non-blocking variant using GDScriptFunctionState but that's complex
    }

    /// Adds a custom command. If you add a command called `foo`, you can call it later by writing `:foo` in the console.
    #[func(gd_self)]
    pub fn add_custom_command(
        mut this: Gd<Self>,
        name: String,
        func: Callable,
        #[opt(default = "")] help: GString, // This has to be GString, not String
    ) -> godot::global::Error {
        // delegate to custom.rs
        this.bind_mut().add_custom_command_inner(name, func, help)
    }

    /// Enter lockdown mode permanently. In lockdown mode, only clap commands can be used, no `GDScript`.
    /// Once lockdown mode is enabled, it cannot be turned off again.
    #[func]
    pub fn lockdown() -> Variant {
        match enable_lockdown() {
            Ok(()) => Variant::from("Lockdown mode enabled."),
            Err(err) => {
                tracing::warn!(?err);
                Variant::from(godot::global::Error::ERR_ALREADY_EXISTS)
            }
        }
    }

    /// Enter VR mode. This will spawn a Sprite3D in front of the camera showing the console.
    /// You can control its size and other parameters by typing `:cons vr`.
    #[func(gd_self)]
    pub fn enter_vr(this: Gd<Self>) -> Variant {
        // May need ensure_ready() here?
        match enter_vr(AsyncGd(this)) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(?err);
                Variant::from(godot::global::Error::ERR_ALREADY_EXISTS)
            }
        }
    }

    /// Call this in any public-facing method that uses any `OnReady<>` field to ensure people don't call it before they're ready.
    /// Else the `OnReady<>` fields will panic when you try to use them.
    fn ensure_ready(&self, method_name: &str) -> Result<(), Report> {
        if !self.base().is_node_ready() {
            // This check prevents a panic later, since we need to use OnReady<> fields on Self
            return Err(eyre!(
                "CrabbyConsole.{method_name}(...) cannot be used before CrabbyConsole._ready() - please wait one frame and try again (or call it like CrabbyConsole.{method_name}.call_deferred(...))",
            ));
        }

        Ok(())
    }

    fn set_search_mode_enabled(&mut self, val: bool) {
        self.search_mode_enabled = val;
        self.suggestions.dirty = true; // causes the autocomplete entries to get regenerated
    }

    fn get_search_mode_enabled(&self) -> bool {
        self.search_mode_enabled
    }

    /// Get all variables defined using :set and :get
    fn get_vars(&self) -> Vec<(StringName, Variant)> {
        self.script_context
            .get_meta_list()
            .iter_shared()
            .map(|meta_name| {
                let value = self.script_context.get_meta(&meta_name);
                (meta_name, value)
            })
            .collect::<Vec<_>>()
    }
}

impl AsyncNode for CrabConsole {
    fn get_executors(&self) -> &TaskGroups<Self> {
        &self.executors
    }
}

#[godot_dyn]
impl ConsoleDebug for CrabConsole {}

// Little helper struct to reduce code repetition.
struct EventChecker<'a> {
    event: &'a InputEvent,
    line_edit: Gd<LineEdit>,
}

impl<'a> EventChecker<'a> {
    fn is(&self, action: &str, allow_echo: bool) -> bool {
        self.is_with_cond(action, allow_echo, true)
    }

    fn is_with_cond(&self, action: &str, allow_echo: bool, base_cond: bool) -> bool {
        (base_cond
            && self
                .event
                .is_action_pressed_ex(action)
                .exact_match(true)
                .allow_echo(allow_echo)
                .done())
        .tap(|cond| {
            if *cond {
                // Prevent key combinations like Tab and arrow keys from having side effects
                self.line_edit.clone().accept_event();
            }
        })
    }
}
