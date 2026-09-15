use std::{num::NonZeroU64, path::PathBuf, sync::OnceLock};

use clap::{Args, Parser, Subcommand, ValueEnum, ValueHint, builder::PossibleValue};
use clap_complete::ArgValueCompleter;
use godot::{
    classes::{display_server::WindowMode, viewport::Msaa},
    prelude::*,
};

use crate::{
    autocomplete::{key_completer, nodepath_completer, resource_completer, scene_file_completer},
    clap_util::{
        BoolArg, POSITION_ARG_HELP, PositionArg, marker, nonzero_finite_num, parse_vec3,
        positive_finite_num, validate_no_braces,
    },
    command::CommandAction,
    delta::DeltaArgs,
    draw::DrawAction,
    plot::PlotAction,
    smooth::SmoothArgs,
    tween::TweenAction,
    vr::VrAction,
};
pub const CLAP_COMMAND_PREFIX: &str = ":"; // just like sqlite3 and vim

// Protip 1: use this to override the help: https://docs.rs/clap/latest/clap/struct.Command.html#method.help_template
// Protip 2: use #[arg] instead of #[clap], see https://github.com/clap-rs/clap/discussions/5262
// Protip 3: always use f64 in Command, so we can use the same combinators
// Protip 4: keep all command names in MainCommand as short as possible!
// Protip 5: you can separate short help from long help by putting a blank line between them in the rustdoc.

/// The root command to control the console.
///
/// Use `--help` for long help, `-h` for short help.
#[derive(Parser)]
#[command(version, no_binary_name = true)]
#[command(name = CLAP_COMMAND_PREFIX)]
pub enum MainCommand {
    /// Clear the console output
    Clear,

    /// Show the user guide
    Guide,

    /// Print text
    #[command(add = marker::Experimental)]
    Print {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        text: Vec<String>, // TODO parse this as expression instead and evaluate it

        /// If set, print the text to stderr instead of stdout
        #[arg[short, long]]
        error: bool,
    },

    /// Control console properties
    #[command(subcommand)]
    Cons(ConsoleAction),

    /// Tweak async stuff
    #[command(subcommand)]
    Async(AsyncAction),

    /// Sleep for the given duration asynchronously (without blocking the main thread)
    Asleep {
        /// The duration to sleep asynchronously for (in seconds)
        #[arg(value_parser = positive_finite_num::<f64>)]
        idle_time: f64,

        /// If set, will also sleep synchronously (blocking the main thread) for the given duration (in seconds, rounded to nearest millisecond)
        #[arg(value_parser = positive_finite_num::<f64>, short, long)]
        busy_time: Option<f64>,

        /// The sleep mode to use. Use Tokio if you want real time. Use Godot if you want the duration to be affected by time scale, lag, and delta smoothing.
        #[arg(value_enum, default_value_t)] // <-- delegates to SleepMode::default()
        sleep_mode: SleepMode,
    },

    /// Sleep for the given duration - this blocks the main thread, so you probably want to use :asleep instead
    Sleep {
        /// The duration to sleep for (in seconds, rounded to the nearest millisecond)
        #[arg(value_parser = positive_finite_num::<f64>)]
        duration: f64,
    },

    /// Toggles pause
    Pause,

    /// Quit immediately
    #[command(visible_aliases = ["exit"])]
    Quit {
        /// Exit code to quit with
        #[arg(default_value_t = 0)]
        exit_code: u8,
    },

    /// Beep beep
    Beep,

    /// Node-specific actions
    #[command(subcommand)]
    Node(NodeAction),

    /// Resource-specific actions
    #[command(subcommand)]
    Res(ResourceAction),

    /// Reload the main scene
    Reload,

    /// Restart the game (does not work when running the game in the editor)
    Restart,

    /// Switch to another scene
    Load {
        /// The path of the scene, e.g. `res://main.tscn`
        #[arg(add = ArgValueCompleter::new(scene_file_completer))]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        path: Vec<String>,

        /// If set, use `ResourceLoader` to load the scene on a background thread. May be faster and reduce lag spikes, but could cause instability or mem leaks, so be careful!
        #[arg(short, long)]
        threaded: bool,
    },

    /// Get/set time scale (1.0 = normal, 2.0 = double speed)
    #[command(allow_negative_numbers = true)]
    Speed {
        /// If set, sets the time scale
        scale: Option<f32>,
    },

    /// Control window properties
    #[command(subcommand)]
    Win(WindowAction),

    /// Control camera properties
    #[command(subcommand, add = marker::Experimental)]
    Cam(CameraAction),

    /// Tween-related commands
    #[command(subcommand, add = marker::Experimental)]
    Tween(TweenAction),

    /// Key-related actions
    #[command(subcommand)]
    Key(KeyAction),

    // ------------ VARIABLES ------------
    /// Toggle Rust flags
    #[command(subcommand, add = marker::Experimental)]
    Flag(FlagAction),

    /// Manage watches. A watch is an expression that gets evaluated every frame (or spread over multiple frames, if async)
    #[command(subcommand)]
    Watch(WatchAction),

    /// Set (or unset) a variable to an expression: `:set a 6*2 + 1`
    Set {
        /// The variable name
        name: String, // TODO use a type-level check to ensure valid value here

        /// The value expression. Use `null` to unset the variable.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        value: Vec<String>,
    },

    /// Get the value of a variable
    Get {
        /// The variable name
        name: String,
    },

    /// List all variables
    Vars,

    // ------------ CONTROL FLOW ------------
    /// Repeat the given async command infinitely (sync commands are not supported, otherwise you'll get an infinite loop)
    Repeat {
        /// The commands to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        commands: Vec<String>,
    },

    /// Run multiple commands in a row, separated by `|>`.
    Seq {
        // TODO add collect: bool here
        /// If set, ignores errors and continue the sequence even if one of the steps errored
        #[arg(short, long)]
        ignore_errors: bool,

        /// The commands to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        commands: Vec<String>,
    },

    /// Run multiple commands concurrently, separated by `<>`, and collect the results.
    Par {
        /// The commands to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        commands: Vec<String>,
    },

    /// Perform an integer for loop, e.g. `:for i 1 10 print({i})`
    For {
        /// Variable binding for the for loop
        #[arg(value_parser = validate_no_braces)]
        binding: String,

        /// Value to start at
        #[arg(allow_negative_numbers = true)]
        from: i64,

        /// Value to end at (inclusive)
        #[arg(allow_negative_numbers = true)]
        to: i64,

        /// Step size (cannot be 0)
        #[arg(
            short,
            long,
            default_value_t = 1,
            allow_negative_numbers = true,
            value_parser = nonzero_finite_num::<i64> // 0 is not allowed because it causes an infinite loop
        )]
        step: i64,

        /// The expression to evaluate. Use e.g. {var} to use the for loop binding, if you named it `var`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        expression: Vec<String>,
    },

    /// Repeatedly run a command until it matches the given condition, e.g. `:select i i.is_valid_int() =-> :key await`. Ensure the expression body is async, or you may get an infinite loop!
    Select {
        /// Name of variable to bind expression result to
        binding: String,

        /// The condition and expression, separated by =->
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        expression: Vec<String>,
        // TODO add --max-iterations here, limits the amounts of consecutive sync iterations (not async, or it may randomly stop if you use it as a watch)
    },

    // ------------ ACCUMULATORS ------------
    /// Calculate the difference between the value of an expression compared to the last time `:delta` was called on it. Note: this only works for numeric types, such as int, float, bool, Vector2, Vector3, etc. Values are remembered for up to 10 minutes, to avoid leaking memory.
    Delta(DeltaArgs),

    /// Smooths out a numeric value over time
    Smooth(SmoothArgs),

    /// Generate a plot to quickly visualize some data
    #[command(subcommand)]
    Plot(PlotAction),

    // ------------------------
    /// Log stuff
    #[command(subcommand)]
    Log(LogAction),

    // ------------ PERFORMANCE ------------
    /// Performance related stuff
    #[command(subcommand)]
    Perf(PerfAction),

    /// Measure the time it takes to run the given expression (in seconds)
    Profile(ProfileArgs),

    /// Return information about the system in a dictionary
    Sysinfo {
        /// If set, pretty print the dictionary and return it as a string
        #[arg(short, long)]
        pretty: bool,
    },

    // ------------ DEBUG ------------
    /// Debug stuff
    #[command(subcommand)]
    Debug(DebugAction),

    /// Debug drawing
    #[command(subcommand)]
    Draw(DrawAction),

    // ------------------------
    /// Evaluate all commands in a text file, separated by newline
    #[command(add = marker::Risky, add = marker::Experimental)]
    EvalFile {
        /// The relative path of the file to evaluate
        #[arg(value_hint = ValueHint::FilePath)]
        filepath: PathBuf,
    },

    /// Manage custom commands
    #[command(subcommand)]
    Cmd(CommandAction),

    /// Open specific folders in the file manager
    Open {
        /// The folder to open
        folder: FolderType,
    },

    // ------------ DIALOGS ------------
    /// Show a dialog and wait asynchronously until it is closed
    Dialog {
        /// The expression to show in the dialog
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        expression: Vec<String>,
    },

    /// Show a prompt that asks the user to type a string, and wait asynchronously until it is closed. Returns the text that was typed, or nil if the dialog was closed
    Prompt {
        /// The expression to show in the prompt
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        expression: Vec<String>,
    },

    // ------------------------
    /// Take a screenshot and save it to the game's userdata folder
    Screenshot,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum FolderType {
    Userdata,
    Logs,
    Screenshots,
}

//FlagAction
#[derive(Subcommand)]
pub enum FlagAction {
    /// Set or toggle flag
    Set {
        /// The name of the flag
        name: String,

        /// The value to set the flag to
        value: BoolArg,
    },

    /// Get flag
    Get {
        /// The name of the flag
        name: String,
    },

    /// List all flags
    List,
}
#[derive(Subcommand)]
pub enum WatchAction {
    /// Start watching an expression
    Add {
        /// The name of the watch
        name: String,

        /// The expression to watch
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        expression: Vec<String>,

        /// If set, the maximum amount of times to evaluate the expression per second (bounded by frame rate). This can be useful to reduce CPU and memory usage
        #[arg(short, long)]
        rate: Option<f64>,
    },

    /// Remove a watch
    #[command(alias = "rm")]
    Remove {
        /// Remove the watch that start with the given prefix
        prefix: String,

        /// If set, remove ALL watches that start with the prefix, instead of just the first
        #[arg(short, long)]
        all: bool,
    },

    /// List all watches and their expressions
    List,

    /// Clear all watches
    Clear,

    /// Refresh all watches (that means: recreate them all from scratch, which is useful after reloading the scene to fix stale node references)
    Refresh, // Once we merge the branch "await_untyped_signal_node_free_detection_attempt" we may not need this anymore?

    // Toggle pause on all watches
    Pause,

    /// Watch all signals on result of the given expression (the expression should return Object)
    Signals {
        /// The Expression to evaluate
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        expression: Vec<String>,

        /// If set, looks up the found object using its instance ID instead of using `get_node()`. Should be way faster, but the watch will break if you reload the main scene. (Note - this flag is only used if the expression returned a Node, else the flag is forced to be always true)
        #[arg(short, long)]
        use_instance_id: bool,

        /// The style of the watch name to use
        #[arg(short, long, value_enum, default_value_t)]
        style: WatchNameStyle,
    },
}

#[derive(ValueEnum, Clone, Debug, Default)]
pub enum WatchNameStyle {
    #[default]
    Name, // MyNode.signal
    NameWithId, // MyNode$33453.signal
    Path,       // /root/MainScene/MyNode.signal
}

#[derive(Subcommand)]
pub enum LogAction {
    /// Set/get the log level
    Level {
        /// If set, set the log level. Can be `trace`, `debug`, `info`, `warning` or `error`, or something fancier like `warn,godot=trace` to set the log level of a specific Rust crate.
        level: Option<String>,
    },

    /// Set/get the log hook. If enabled, it will register a Logger in Godot's to hook its output and print it in the console.
    Hook {
        /// Whether to enable/disable/toggle the log hook
        enabled: Option<BoolArg>,
    },
}

#[derive(Subcommand)]
pub enum PerfAction {
    /// Get current framerate
    Fps,

    /// Get memory usage of the game in MB. Note: this is slow on Windows (5ms+), so don't call this every frame
    Mem {
        /// If set, return bytes instead of megabytes
        #[arg(short, long)]
        bytes: bool,
    },

    /// Get total (uncached) disk read/write in MB since program started. Feed it into `:delta --dt` to get the current read/write speed in MB/s
    Disk {
        /// If set, return bytes instead of megabytes
        #[arg(short, long)]
        bytes: bool,

        #[arg(value_enum)]
        mode: DiskUsageMode,
    },

    /// Get the time since the last frame (in seconds)
    FrameTime {
        /// If set, return milliseconds instead of seconds
        #[arg(long)]
        ms: bool,
    },
}

#[derive(Clone, ValueEnum)]
pub enum DiskUsageMode {
    Read,
    Write,
}

#[derive(Args)]
pub struct ProfileArgs {
    /// If set, return milliseconds instead of seconds
    #[arg(long)]
    pub ms: bool,

    /// The expression to evaluate
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
    pub expression: Vec<String>,
}

#[derive(Subcommand)]
pub enum DebugAction {
    /// Allocate a zero-filled memory buffer and then async sleep a bit (useful for measuring memory speed and memory diagnostics)
    #[command(add = marker::Risky, add = marker::Experimental)]
    Allocate {
        /// Size of the buffer to allocate (in bytes)
        size: usize,
    },

    /// Write a buffer to disk (useful for testing whether Tokio io blocks the main thread)
    #[command(add = marker::Risky, add = marker::Experimental)]
    Write {
        /// Size of the buffer to write (in bytes)
        size: usize,
    },

    /// Perform a GET web request
    #[command(add = marker::Risky, add = marker::Experimental)]
    HttpGet {
        /// The url to request
        url: String,
    },

    /// Debug-print a node using its `Debug` impl. By default prints information about `CrabConsole`.
    Print {
        /// Print everything on one line
        #[arg(short, long)]
        single_line: bool,

        /// Node pattern to find and call the Debug impl on. Must be a Rust-based class that implements the `ConsoleDebug` trait, or it will fail.
        pattern: Option<String>,
    },

    /// Panic immediately
    Panic,

    /// Return an error
    Error,
}

#[derive(Subcommand)]
pub enum WindowAction {
    /// Get/set `VSync`
    Vsync {
        /// Whether to enable `VSync`
        enabled: Option<BoolArg>,
    },
    /// Set/get the window size
    Size {
        /// Width in pixels
        width: Option<u16>,
        /// Height in pixels
        height: Option<u16>,
    },
    /// Set the window mode
    Mode {
        /// The window mode
        mode: WindowModeArg,
    },

    /// Set anti-aliasing level
    Msaa {
        /// The anti-aliasing level
        level: MsaaArg,
    },
}

#[derive(Clone)]
pub struct WindowModeArg(pub WindowMode);

impl ValueEnum for WindowModeArg {
    fn value_variants<'a>() -> &'a [Self] {
        // Little hack - since Clap forces us to return a &[] here, we can't just return a temporary Vec.
        // That would fail to compile with "returning reference to temporary".
        // So we need to store it globally.
        static VARIANTS: OnceLock<Vec<WindowModeArg>> = OnceLock::new();
        VARIANTS.get_or_init(|| {
            WindowMode::values()
                .iter()
                .map(|&m| WindowModeArg(m))
                .collect()
        })
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(PossibleValue::new(self.0.as_str().to_lowercase()))
    }
}

#[derive(Subcommand)]
pub enum CameraAction {
    /// Set camera projection mode
    #[command(visible_aliases = ["proj"])]
    Projection {
        /// The camera projection mode
        mode: CameraProjectionArg,
    },

    /// Move camera to given position/rotation
    #[command(visible_aliases = ["mv"])]
    Move {
        /// Position to move camera to
        #[arg(value_parser = parse_vec3)]
        pos: Vector3,

        /// Rotation to rotate camera to (in degrees)
        #[arg(value_parser = parse_vec3)]
        rot: Option<Vector3>,
    },

    /// Set camera FOV
    Fov {
        /// Camera FOV to set camera to
        fov: f32,
    },

    /// Set camera zclip
    Zclip {
        /// Near clipping plane
        near: f32,
        /// Far clipping plane
        far: f32,
    },

    /// Return info about the camera state - useful for animation.
    #[command(visible_aliases = ["i"])]
    Info {
        /// Print the raw transform instead of rotation/origin
        #[arg(short, long)]
        transform: bool,
    },
}

#[derive(Clone)]
pub enum CameraProjectionArg {
    Perspective,
    Orthogonal,
    Toggle,
}

impl ValueEnum for CameraProjectionArg {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Perspective, Self::Orthogonal, Self::Toggle]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            Self::Perspective => PossibleValue::new("perspective").aliases(["pers"]),
            Self::Orthogonal => PossibleValue::new("orthogonal").aliases(["ortho"]),
            Self::Toggle => PossibleValue::new("toggle"),
        })
    }
}

/// Mirrors Godot's Msaa enum
#[derive(ValueEnum, Clone)]
pub enum MsaaArg {
    #[value(name = "0")]
    Disabled,
    #[value(name = "2")]
    Msaa2x,
    #[value(name = "4")]
    Msaa4x,
    #[value(name = "8")]
    Msaa8x,
}

impl From<MsaaArg> for Msaa {
    fn from(msaa: MsaaArg) -> Self {
        match msaa {
            MsaaArg::Disabled => Self::DISABLED,
            MsaaArg::Msaa2x => Self::MSAA_2X,
            MsaaArg::Msaa4x => Self::MSAA_4X,
            MsaaArg::Msaa8x => Self::MSAA_8X,
        }
    }
}

#[derive(Subcommand)]
pub enum ConsoleAction {
    /// Control remote console
    #[command(subcommand, add = marker::Risky)]
    Remote(RemoteConsoleAction),

    /// Enable/disable timestamps. When enabled, every line in the console will show a timestamp like `[11:05:56]` in front of it.
    Timestamps {
        /// Whether to enable or disable timestamps
        enabled: Option<BoolArg>,
    },

    /// Control VR mode
    #[command(subcommand, add = marker::Experimental)]
    Vr(VrAction),
}

#[derive(Subcommand)]
pub enum RemoteConsoleAction {
    /// Start the remote console
    Start {
        /// The hostname. Use `127.0.0.1` to bind to localhost on IPv4, `localhost` to bind to `::1` on IPv6, and `0.0.0.0` to bind on all addresses on IPv4 (or `::` for IPv6), so other devices on the network can connect to it (risky).
        #[arg(default_value_t = String::from("127.0.0.1"))]
        host: String,

        /// The port. Use `0` to let the OS assign a random port.
        #[arg(default_value_t = 0)]
        port: u16,
    },

    /// Stops the remote console
    Stop,
}

#[derive(Subcommand)]
pub enum AsyncAction {
    /// Set the async/signal execution timeout for the console
    Timeout {
        /// The async/signal timeout (in seconds)
        #[arg(value_parser = positive_finite_num::<f64>)]
        timeout: f64,
    },

    /// Return a future that never resolves (useful for debugging timeout)
    Pending,
}

#[derive(Clone, ValueEnum, Debug, Default)]
pub enum SleepMode {
    #[default]
    Tokio,
    Godot,
}

#[derive(Subcommand)]
pub enum NodeAction {
    /// Spawns a specific 3D node at the given position and adds it to the main scene
    #[command(add = marker::Risky, add = marker::Experimental)]
    Spawn {
        /// If set, set the name of the instantiated node
        #[arg(short, long)]
        name: Option<String>,

        /// The 3D position to spawn the scene at
        #[arg(short, long, value_name = POSITION_ARG_HELP)]
        pos: Option<PositionArg>,

        /// The path of the scene, e.g. `res://main.tscn`
        #[arg(add = ArgValueCompleter::new(scene_file_completer))]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        path: Vec<String>,
    },

    /// Find a node (or nodes) that matches the given pattern or type
    Find {
        /// The `NodePath` to search for. If empty, find all nodes.
        #[arg(add = ArgValueCompleter::new(nodepath_completer))]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        nodepath_needle: Vec<String>, //                           ^^^^^^^^^^^^^^ <- optional

        /// If set, returns *all* nodes that match the given pattern, instead of just the first one
        #[arg(short, long)]
        all: bool,

        /// If set, returns the full `NodePath` of the node, instead of the node itself
        #[arg(short, long)]
        path: bool,

        /// If set, returns only nodes that inherit the given type
        #[arg(short, long = "type", value_name = "TYPE")]
        typ: Option<String>,
    },

    /// List all nodes
    List,

    /// Reload a node (or nodes) from disk (note - this only works for nodes that are the root node of a scene file stored on disk)
    Reload {
        /// The `NodePath` to search for.
        #[arg(add = ArgValueCompleter::new(nodepath_completer))]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        nodepath_needle: Vec<String>,

        /// If set, reloads *all* nodes that match the given pattern, instead of just the first one
        #[arg(short, long)]
        all: bool,
    },

    /// Find a node by its instance ID
    FromId { instance_id: NonZeroU64 }, // <-- no need to use i64 here, since negative instance id means its RefCounted, and nodes can never be RefCounted

    /// Wait for any signal to come in on a node, and return the signal's data
    Signals {
        /// The `NodePath` to search for.
        #[arg(add = ArgValueCompleter::new(nodepath_completer))]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        nodepath_needle: Vec<String>,
    },
}
#[derive(Subcommand)]
pub enum ResourceAction {
    /// Find resources by name
    Find {
        /// Finds resources according to the given needle (if empty, finds ALL resources)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        needle: Vec<String>, //                                    ^^^^^^^^^^^^^^ => optional

        /// If set, returns *all* resources that match the needle, instead of just the first
        #[arg(short, long)]
        all: bool,

        /// If set, loads the resource(s) instead of just returning its file path (note - cannot be combined with --all)
        #[arg(short, long)]
        load: bool,
    },

    /// Load a specific resource. To store it in a variable, you can do e.g. `:set icon :res load res://icon.png`
    Load {
        /// The path of the resource, e.g. `res://icon.png`
        #[arg(add = ArgValueCompleter::new(resource_completer))]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        path: Vec<String>,

        /// If set, use `ResourceLoader` to load the resource on a background thread. May be faster and reduce lag spikes, but could cause instability or mem leaks, so be careful!
        #[arg(short, long)]
        threaded: bool,
    },
}

//BindAction
#[derive(Subcommand)]
pub enum KeyAction {
    /// Bind key to a command
    Bind {
        /// Key to bind
        #[arg(add = ArgValueCompleter::new(key_completer))]
        key: String,

        /// Command to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        command: Vec<String>,
    },

    /// Unbind key
    Unbind {
        /// Key to unbind
        #[arg(add = ArgValueCompleter::new(key_completer))]
        key: String,
    },

    /// List all bindings
    List {
        /// List all possible keys you can bind a command to
        // (do not remove this command, it's useful for debugging and binding a command to every single key)
        #[arg(short, long)]
        valid_keys: bool,
    },

    /// Clear all bindings
    Clear,

    /// Wait for an (unhandled) key to be pressed (so, it will not work while the text box is focused, since the text box consumes the events)
    Await {
        /// If set, key to wait for (else waits for any key)
        #[arg(add = ArgValueCompleter::new(key_completer))]
        key: Option<String>,
        // TODO: add --handled here to wait for handled events?
    },
}
