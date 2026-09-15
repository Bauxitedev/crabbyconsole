//! The Clap expression evaluator lives in this module.

use std::{
    collections::HashMap, future::pending, io::ErrorKind, path::Path, rc::Rc, sync::Arc,
    time::Duration,
};

use clap::{Arg, ArgAction, Command, CommandFactory, FromArgMatches as _, builder::CommandExt};
use color_eyre::{
    Report, Result as EyreResult,
    eyre::{self, Context as _, OptionExt as _, ensure, eyre},
};
use crabbyconsole_clap::{
    clap_util::{BoolArg, marker},
    eval_definition::{
        AsyncAction, CameraAction, CameraProjectionArg, ConsoleAction, DebugAction, DiskUsageMode,
        FlagAction, FolderType, KeyAction, LogAction, MainCommand, PerfAction, ProfileArgs,
        RemoteConsoleAction, ResourceAction, SleepMode, WindowAction,
    },
};
use crabbyconsole_misc::{
    FutureTracyExt as _,
    async_util::{
        FutureKind, classify_future, wait_for_any_key_pressed, wait_for_key_pressed,
        wait_for_next_frame,
    },
    flags::FLAGS_MISC,
    gd::async_node::{AsyncGd, AsyncNode, TOKIO_RUNTIME},
    logging::{get_log_level, set_log_level},
    profile,
    reflection::{ConsoleDebug, OpaqueDebug},
    util::{
        build_key_name_map, get_all_resources, get_camera_3d, get_process_ram_bytes,
        get_process_total_disk_usage_bytes, get_scene_tree, get_viewport,
    },
};
use futures::future::join_all;
use futures_lite::future::or;
use godot::{
    classes::{camera_3d::ProjectionType, display_server::VSyncMode, viewport::Msaa, *},
    global::{Error as GodotError, Key},
    prelude::*,
};
use tokio::io::AsyncBufReadExt as _;

use crate::gd::console::{
    CrabConsole, CustomCommand,
    eval::{ClapSubAction as _, custom::CustomCommandExpression},
    job::{EvalClapError, JobExpressionCallableResult, JobExpressionInner},
    lockdown::is_lockdown_enabled,
    util::{
        SCREENSHOTS_FOLDER, get_sysinfo, http_get, is_valid_variable_name, load_threaded,
        render_template, show_dialog, show_prompt, split_console_whitespace, stepped_range,
        take_screenshot, write_zeroes,
    },
};

impl CrabConsole {
    pub(crate) fn inject_subcommands(self: &AsyncGd<Self>, mut cmd: Command) -> Command {
        for (name, CustomCommand { help, .. }) in &self.bind().commands {
            cmd = cmd.subcommand(
                Command::new(name.to_string())
                    .about(help) // note - about() covers both short help (-h) and long help (--help)
                    .trailing_var_arg(true)
                    .arg(
                        Arg::new("args") // Add a single variadic argument that allows 0 or more entries
                            .num_args(0..)
                            .action(ArgAction::Append)
                            .allow_hyphen_values(true)
                            .value_name("ARGS"),
                    ),
            )
        }
        cmd
    }
    #[tracing::instrument(skip_all)]
    pub(super) async fn eval_clap(
        self: AsyncGd<Self>,
        clap: &str,
    ) -> Result<Variant, EvalClapError> {
        let split = split_console_whitespace(clap);

        // Clap has some commands specific to repls: see:
        // - https://docs.rs/clap/latest/clap/struct.Command.html#method.multicall
        // - https://docs.rs/clap/latest/clap/struct.Command.html#method.no_binary_name

        let mut cmd = MainCommand::command();
        cmd = self.inject_subcommands(cmd);

        // TODO cloning Command here may be slow - check profiler!
        // Needed due to new riskiness check
        let matches = cmd.clone().try_get_matches_from(split)?;

        if let Some((name, matches)) = matches.subcommand() {
            // clone here prevents long bind.
            // do NOT use if let x = y.bind().z.clone() or it will panic with double borrow
            let cmd_slot = self.bind().commands.get(name).cloned();

            if let Some(CustomCommand { expression, .. }) = cmd_slot {
                // Parse arguments
                let args: Vec<String> = matches
                    .get_many::<String>("args")
                    .unwrap_or_default()
                    .cloned() // &String -> String
                    .collect();

                return Ok(self.handle_custom_command(expression, args).await?);
            }
        }

        // If lockdown mode is enabled, reject risky commands
        if is_lockdown_enabled() {
            let (is_risky, risky_cmd) = check_marker::<marker::Risky>(&cmd, &matches);
            if is_risky {
                return Err(EvalClapError::Lockdown { risky_cmd });
            }
        }

        let args = MainCommand::from_arg_matches(&matches)?;

        Ok(self.handle_clap_command(args).await?)
    }

    pub(super) async fn handle_custom_command(
        self: &AsyncGd<Self>,
        custom_expression: CustomCommandExpression,
        args: Vec<String>,
    ) -> EyreResult<Variant> {
        let expression = match custom_expression {
            CustomCommandExpression::String(string) => JobExpressionInner::String(string),
            CustomCommandExpression::Callable(func) => {
                let args_len = args.len();

                let closure = move || -> JobExpressionCallableResult {
                    let func = func.clone(); // needed otherwise it becomes FnOnce

                    // Note; do not parse the strings into Variant::number etc, leave that up the user.
                    let callable_args = args
                        .iter()
                        .map(|s| Variant::from(s.clone()))
                        .collect::<VarArray>();

                    Box::pin(async move {
                        // NOTE: If called on an invalid Callable then no error is printed, and NIL is returned.
                        // i think the problem is the closure is destroyed when run_12345() exits, so the callable refers to a destroyed closure
                        // so closures may not work, since they get destroyed too early, but bind() does seem to work, so use that instead
                        if !func.is_valid() {
                            return Err(eyre!("Callable is not valid - did it get freed?"));
                        }

                        // Warning - If called with fewer arguments than expected, callv will crash Godot (without triggering UB).
                        // However, if you pass too many, the rest will be ignored.
                        // This is convenient, because variadic functions only return the amount of non-variadic arguments they take.
                        // So, check with < instead of !=, otherwise we can't call variadic functions properly.

                        // TODO - this does NOT work properly with arguments that can have default values, e.g.
                        //      > combine(name: String, greeting: String = "Hello", ...extras: Array)
                        // Because func.get_argument_count() will return 2, not 1, so you can never take advantage of the default argument.

                        // Note that get_argument_count returns 1 for print() (you would expect 0 because variadic but eh).
                        // Also, luckily get_argument_count does seem to work properly for bound functions.
                        // I haven't tested bind + variadic + optional args yet though.
                        let callable_arg_count = func.get_argument_count();
                        if args_len < callable_arg_count {
                            return Err(eyre!(
                                "Callable expected at least {} arguments, but got only {}",
                                callable_arg_count,
                                args_len
                            ));
                        }

                        // TODO we can do basic typechecking here maybe?
                        // if callable.object() is Some you can lookup callable.get_method() in object's ClassDB (or get_method_list()) to get type information
                        // note - the argument list may not line up 1-to-1 with get_method_list().
                        // e.g. if you have a method  Object.foo(a, b) and do Object.foo.bind("a") then you may need to skip the first arg in Object.get_method_list("foo")

                        Ok(func.callv(&callable_args))
                    })
                };

                JobExpressionInner::Callable(OpaqueDebug(Rc::new(closure)))
            }
        };

        let result = self
            .clone()
            .eval_job_without_channel(expression)
            .with_tracy_non_continuous_frame("handle_custom_command")
            .await?;

        Ok(result)
    }
    pub(super) async fn handle_clap_command(
        mut self: AsyncGd<Self>,
        command: MainCommand,
    ) -> EyreResult<Variant> {
        match command {
            MainCommand::Clear => {
                self.bind_mut()
                    .bound_task()
                    .new(async |mut this| {
                        // Wait 1 frame and then clear it, otherwise it's not fully cleared, since the result comes in later.
                        wait_for_next_frame().await;
                        this.bind_mut().nodes.console_history.clear();
                    })
                    .spawn();

                Ok(Variant::nil())
            }

            MainCommand::Guide => {
                let guide_text = self.bind().initial_rich_text.clone();

                self.bind_mut()
                    .nodes
                    .console_history
                    .append_text(&guide_text); // BBcode-parsed
                Ok(Variant::nil())
            }

            MainCommand::Print { text, error } => {
                let text = text.join(" ");
                //godot_print!("{text}"); // <-- probably slower, since it formats the string

                if error {
                    godot::global::printerr(&[Variant::from(text)]);
                } else {
                    godot::global::print(&[Variant::from(text)]);
                }

                Ok(Variant::nil())
            }

            MainCommand::Cons(action) => match action {
                ConsoleAction::Remote(action) => {
                    match action {
                        RemoteConsoleAction::Start { host, port } => {
                            // Create these futures first, to avoid missing the emission of the signal.
                            // (Calling to_future() wires up the signal.)
                            let started = self
                                .bind_mut()
                                .signals()
                                .remote_console_started()
                                .to_future();
                            let start_failed = self
                                .bind_mut()
                                .signals()
                                .remote_console_start_failed()
                                .to_future();

                            self.bind_mut()
                                .signals()
                                .remote_console_start_requested()
                                .emit(&host, port);

                            // Wait for either remote console to start, or fail to start.
                            or(
                                async move {
                                     // Separate this into a new line to prevent long-lasting bind
                                    let (ip, port) = started.await;

                                    let result_str = format!(
                                        "Listening on {ip}:{port} - connect with: rlwrap nc {ip} {port}"
                                    );

                                    Ok(Variant::from(result_str))
                                },
                                async move {
                                    let (err,) = start_failed.await;
                                    Err(eyre!("Failed to start remote console: {err}"))
                                },
                            )
                            .await
                        }

                        RemoteConsoleAction::Stop => {
                            self.bind_mut()
                                .signals()
                                .remote_console_stop_requested()
                                .emit();

                            Ok(Variant::nil())
                        }
                    }
                }

                ConsoleAction::Timestamps { enabled } => {
                    if let Some(enabled) = enabled {
                        let new_val = match enabled {
                            BoolArg::Set(enabled) => {
                                self.bind_mut().timestamps_enabled = enabled;
                                enabled
                            }
                            BoolArg::Toggle => {
                                self.bind_mut().timestamps_enabled ^= true;
                                self.bind().timestamps_enabled
                            }
                        };

                        Ok(Variant::from(new_val))
                    } else {
                        Ok(Variant::from(self.bind().timestamps_enabled))
                    }
                }

                ConsoleAction::Vr(action) => action.handle(self.clone()).await,
            },

            MainCommand::Async(action) => match action {
                AsyncAction::Timeout { timeout } => {
                    self.bind_mut().timeout = timeout;
                    Ok(Variant::nil())
                }

                AsyncAction::Pending => {
                    pending::<()>().await;
                    Ok(Variant::nil())
                }
            },

            MainCommand::Asleep {
                idle_time,
                busy_time,
                sleep_mode,
            } => {
                if let Some(busy_time) = busy_time {
                    Os::singleton().delay_msec((busy_time * 1000.0).round() as i32);
                }
                match sleep_mode {
                    SleepMode::Tokio => {
                        //let _guard = TOKIO_RUNTIME.enter(); // <-- don't do this, causes panic if called in parallel

                        TOKIO_RUNTIME
                            .spawn(async move {
                                tokio::time::sleep(Duration::from_secs_f64(idle_time)).await;
                            })
                            .await
                            .unwrap();

                        Ok(Variant::from(format!("Slept {idle_time}s using Tokio")))
                    }
                    SleepMode::Godot => {
                        get_scene_tree()
                            .create_timer(idle_time)
                            .signals()
                            .timeout()
                            .to_future()
                            .await;

                        Ok(Variant::from(format!("Slept {idle_time}s using Godot")))
                    }
                }
            }

            MainCommand::Sleep { duration } => {
                Os::singleton().delay_msec((duration * 1000.0).round() as i32);
                Ok(Variant::nil())
            }

            MainCommand::Pause => {
                get_scene_tree().set_pause(!get_scene_tree().is_paused());
                Ok(Variant::nil())
            }

            MainCommand::Quit { exit_code } => {
                get_scene_tree()
                    .quit_ex()
                    .exit_code(exit_code as i32)
                    .done();
                Ok(Variant::nil())
            }

            MainCommand::Beep => {
                DisplayServer::singleton().beep();
                Ok(Variant::from("*beep*"))
            }

            MainCommand::Node(action) => action.handle(self.clone()).await,

            MainCommand::Res(action) => match action {
                ResourceAction::Find { needle, all, load } => {
                    let resources = get_all_resources();

                    // TODO it should only return the first one by default
                    // otherwise it's inconsistent with :node find
                    let needle = if needle.is_empty() {
                        None
                    } else {
                        Some(needle.join(" "))
                    };

                    // TODO it seems to contain all files in the project folder, not just the ones that are actually used in godot...

                    let mut list = resources
                        .into_iter()
                        .filter(|res| {
                            if let Some(needle) = &needle {
                                res.to_lowercase().contains(&needle.to_lowercase())
                            } else {
                                true
                            }
                        })
                        .collect::<Vec<_>>();

                    if load {
                        // If `load` is set, we ignore the `all` flag.
                        // Because loading every resource could cause a massive spike in memory usage.
                        let first = list
                            .into_iter()
                            .next()
                            .ok_or_else(|| eyre!("No resource found that matches given needle"))?;
                        Ok(Variant::from(godot::tools::try_load::<Resource>(&first)?))
                    } else {
                        if !all {
                            list.truncate(1); // If !all, return only the first match
                        }

                        Ok(Variant::from(list.join("\n")))
                    }
                }

                ResourceAction::Load { path, threaded } => {
                    let path = path.join(" ");

                    let res = if threaded {
                        load_threaded(&path).await?
                    } else {
                        try_load::<Resource>(&path)?
                    };

                    Ok(Variant::from(res))
                }
            },

            MainCommand::Reload => {
                get_scene_tree().reload_current_scene();
                Ok(Variant::nil())
            }

            MainCommand::Restart => {
                // Note - set_restart_on_exit does NOT work when project is started from the editor.

                // Note 2 - Godot seems to steal some CLI args such as `--fixed-fps`.
                // They will NOT appear in get_cmdline_args.

                let mut os = Os::singleton();

                let args = {
                    let mut args = os.get_cmdline_args();
                    args.push("--");
                    args.extend_array(&os.get_cmdline_user_args());
                    tracing::info!(?args, "restarting...");
                    args
                };
                os.set_restart_on_exit_ex(true).arguments(&args).done();
                get_scene_tree().quit();
                Ok(Variant::nil())
            }

            MainCommand::Load { path, threaded } => {
                // This is equivalent to get_scene_tree().change_scene_to_file("res://main.tscn")
                // except with better error printing + it returns the PackedScene

                let path = path.join(" ");
                let scene = if threaded {
                    load_threaded(&path)
                        .await?
                        .try_cast::<PackedScene>()
                        .map_err(|_| {
                            eyre!("failed to cast loaded resource at `{path}` to PackedScene")
                        })?
                } else {
                    try_load::<PackedScene>(&path)?
                };

                match get_scene_tree().change_scene_to_packed(&scene) {
                    GodotError::OK => Ok(Variant::from(scene)),
                    err => Err(eyre!("{err:?}")),
                }
            }

            MainCommand::Speed { scale } => match scale {
                Some(scale) => {
                    Engine::singleton().set_time_scale(scale as f64);
                    Ok(Variant::nil())
                }
                None => Ok(Variant::from(Engine::singleton().get_time_scale())),
            },

            MainCommand::Win(action) => {
                let mut window = self.gd().get_window().expect("no game window");
                let mut ds = DisplayServer::singleton();
                let mut vp = get_viewport();

                match action {
                    WindowAction::Vsync { enabled } => {
                        if let Some(enabled) = enabled {
                            let new_value = match enabled {
                                BoolArg::Set(value) => {
                                    if value {
                                        VSyncMode::ENABLED // TODO allow changing to other vsync modes maybe?
                                    } else {
                                        VSyncMode::DISABLED
                                    }
                                }
                                BoolArg::Toggle => match ds.window_get_vsync_mode() {
                                    VSyncMode::DISABLED => VSyncMode::ENABLED,
                                    _ => VSyncMode::DISABLED,
                                },
                            };

                            ds.window_set_vsync_mode(new_value);
                            Ok(Variant::from(format!("VSync mode set to {new_value:?}")))
                        } else {
                            Ok(Variant::from(ds.window_get_vsync_mode()))
                        }
                    }
                    WindowAction::Size { width, height } => {
                        match (width, height) {
                            (None, None) => Ok(Variant::from(window.get_size())),

                            (Some(width), Some(height)) => {
                                // Godot docs recommend using window.set_size instead of DisplayServer.window_set_size

                                window.set_size(Vector2i::new(width as i32, height as i32));
                                Ok(Variant::nil())
                            }

                            _ => Err(eyre!(
                                "Please supply both window width and window height, e.g. `:win size 1280 800`"
                            )),
                        }
                    }
                    WindowAction::Mode { mode } => {
                        ds.window_set_mode(mode.0);
                        Ok(Variant::nil())
                    }
                    WindowAction::Msaa { level } => {
                        let msaa = Msaa::from(level);
                        vp.set_msaa_3d(msaa);
                        Ok(Variant::nil())
                    }
                }
            }

            MainCommand::Cam(action) => {
                // TODO support Camera2D too
                let mut cam = get_camera_3d().ok_or_eyre(
                    "no Camera3D in the scene - this command only supports 3D cameras for now",
                )?;

                match action {
                    CameraAction::Projection { mode } => {
                        let new_proj_mode = match mode {
                            CameraProjectionArg::Perspective => ProjectionType::PERSPECTIVE,
                            CameraProjectionArg::Orthogonal => ProjectionType::ORTHOGONAL,
                            CameraProjectionArg::Toggle => match cam.get_projection() {
                                ProjectionType::ORTHOGONAL => ProjectionType::PERSPECTIVE,
                                _ => ProjectionType::ORTHOGONAL,
                            },
                        };

                        cam.set_projection(new_proj_mode);
                        Ok(Variant::from(new_proj_mode.as_str()))
                    }
                    CameraAction::Move { pos, rot } => {
                        cam.set_global_position(pos);
                        if let Some(rot) = rot {
                            cam.set_global_rotation_degrees(rot);
                        }

                        Ok(Variant::nil())
                    }
                    CameraAction::Fov { fov } => {
                        cam.set_fov(fov);

                        Ok(Variant::nil())
                    }
                    CameraAction::Zclip { near, far } => {
                        cam.set_near(near);
                        cam.set_far(far);

                        Ok(Variant::nil())
                    }
                    CameraAction::Info { transform } => {
                        // Make sure to print it in a Rust-compatible way so we can copy paste it into Rust
                        let info = if !transform {
                            format!(
                                "let rotation = {:?};\nlet origin = {:?};\nlet fov = {};\n",
                                cam.get_global_rotation(), // Since scale is assumed to be 1,1,1
                                cam.get_global_position(),
                                cam.get_fov()
                            )
                        } else {
                            format!(
                                "let transform = {:?};\nlet fov = {};\n",
                                cam.get_global_transform().orthonormalized(), // Since scale is assumed to be 1,1,1
                                cam.get_fov()
                            )
                        };

                        Ok(Variant::from(info))
                    }
                }
            }

            MainCommand::Tween(action) => action.handle(self.clone()).await,

            MainCommand::Key(action) => match action {
                KeyAction::Bind { key, command } => {
                    let key_parsed = lookup_key(&key)?;
                    let command = command.join(" ");

                    self.bind_mut().bindings.insert(key_parsed, command.clone());

                    Ok(Variant::from(format!(
                        "Bound key {key_parsed:?} to {command}",
                    )))
                }
                KeyAction::Unbind { key } => {
                    let key_parsed = lookup_key(&key)?;

                    let old = self.bind_mut().bindings.shift_remove(&key_parsed);

                    match old {
                        Some(old) => Ok(Variant::from(format!(
                            "Unbound key {key_parsed:?} with command {old:?}",
                        ))),
                        None => Err(eyre!("Key {:?} was not bound", key_parsed)),
                    }
                }
                KeyAction::List { valid_keys } => {
                    if valid_keys {
                        let map = build_key_name_map();
                        let keys = map
                            .iter()
                            .map(|(k, _v)| GString::from(k))
                            .collect::<Vec<_>>();

                        Ok(Variant::from(keys))
                    } else {
                        let bindings = &self.bind_mut().bindings;
                        let result = format!("{bindings:#?}",);
                        Ok(Variant::from(result))
                    }
                }
                KeyAction::Clear => {
                    let len = self.bind().bindings.len();
                    self.bind_mut().bindings.clear();

                    Ok(Variant::from(format!("Removed {len} bindings.")))
                }
                KeyAction::Await { key } => {
                    if let Some(key) = key {
                        let key_parsed = lookup_key(&key)?;

                        wait_for_key_pressed(key_parsed).await; // Note - does not wait for echo events
                        Ok(Variant::from(key))
                    } else {
                        let key = wait_for_any_key_pressed().await;

                        Ok(Variant::from(
                            Os::singleton().get_keycode_string(key.get_keycode()),
                        ))
                    }
                }
            },

            MainCommand::Flag(action) => match action {
                FlagAction::Set { name, value } => {
                    let result = match value {
                        BoolArg::Set(val) => FLAGS_MISC.set(&name, val),
                        BoolArg::Toggle => FLAGS_MISC.toggle(&name),
                    }
                    .ok_or_else(|| {
                        eyre!("invalid flag {name} - try `:flag list` to see all flags")
                    })?;

                    Ok(Variant::from(result))
                }
                FlagAction::Get { name } => {
                    Ok(Variant::from(FLAGS_MISC.get(&name).ok_or_else(|| {
                        eyre!("invalid flag {name} - try `:flag list` to see all flags")
                    })?))
                }
                FlagAction::List => {
                    let dict = FLAGS_MISC
                        .iter()
                        .map(|(k, v)| format!("{} => {}", k, v.get()))
                        .collect::<Vec<_>>()
                        .join("\n");

                    // NOTE - variant::from(Dictionary) panics, so generate string instead
                    Ok(Variant::from(dict))
                }
            },

            MainCommand::Watch(action) => action.handle(self.clone()).await,
            MainCommand::Set { name, value } => {
                let expression = value.join(" ");

                let result = self
                    .clone()
                    .eval_job_without_channel(JobExpressionInner::String(expression.into()))
                    .with_tracy_non_continuous_frame("eval_job_set")
                    .await?;

                // TODO encode this in the type system
                // This is needed otherwise you can do :set _ 1 and it will break all future expressions
                ensure!(
                    is_valid_variable_name(&name),
                    "invalid variable name `{name}`, must be alphanumeric, must not start with a digit, and cannot be a reserved keyword"
                );

                self.gd_mut()
                    .bind_mut()
                    .script_context
                    .set_meta(&name, &result);

                Ok(result)
            }

            MainCommand::Get { name } => Ok(self.gd().bind().script_context.get_meta(&name)),

            MainCommand::Vars => {
                let metas = self
                    .gd()
                    .bind()
                    .get_vars()
                    .into_iter()
                    .map(|(meta_name, value)| {
                        // We need parentheses here for some reason?
                        (varray![&meta_name, &value])
                    })
                    .collect::<Vec<VarArray>>();

                Ok(Variant::from(metas))
            }

            MainCommand::Repeat { commands } => {
                let commands = commands.join(" ");
                let commands: Arc<str> = Arc::from(commands);
                loop {
                    let result_body = classify_future(async {
                        self.clone()
                            .eval_job_without_channel(JobExpressionInner::String(Arc::clone(
                                &commands,
                            )))
                            .with_tracy_non_continuous_frame("eval_job_repeat")
                            .await
                    });

                    match result_body {
                        FutureKind::Sync(_) => {
                            return Err(eyre!(":repeat only supports async commands"));
                        }
                        FutureKind::Async(fut) => fut.await?,
                    };
                }
            }

            MainCommand::Seq {
                ignore_errors,
                commands,
            } => {
                let commands: Vec<String> = commands
                    .split(|x| x == "|>") // we should reserve => for :select or :command add sum a b => a+b
                    .filter(|group| !group.is_empty())
                    .map(|group| group.join(" "))
                    .collect();

                let mut result = None;
                for command in commands {
                    let value = self
                        .clone()
                        .eval_job_without_channel(JobExpressionInner::String(command.into()))
                        .with_tracy_non_continuous_frame("eval_job_seq")
                        .await;

                    if !ignore_errors && let Err(ref err) = value {
                        // If an error occurred, and we're not ignoring errors, return early
                        return Err(Arc::clone(err).into());
                    }

                    result = Some(value);
                }

                match result {
                    Some(result) => Ok(result?),
                    None => Ok(Variant::nil()),
                }
            }

            MainCommand::Par { commands } => {
                let commands: Vec<String> = commands
                    .split(|x| x == "<>")
                    .filter(|group| !group.is_empty())
                    .map(|group| group.join(" "))
                    .collect();

                let results = join_all(commands.into_iter().map(|command| {
                    let this = self.clone();
                    async move {
                        this.eval_job_without_channel(JobExpressionInner::String(command.into()))
                            .with_tracy_non_continuous_frame("eval_job_par")
                            .await
                    }
                }))
                .await;

                // Cool trick: you can collect a Vec<Result<T,E>> into a Result<Vec<T>, E>
                // So it will bail out at the first error.
                let results = results.into_iter().collect::<Result<Vec<_>, _>>()?;

                Ok(Variant::from(results))
            }

            MainCommand::For {
                binding,
                from,
                to,
                step,
                expression,
            } => {
                let base_expression = expression.join(" ");
                let range = stepped_range(from, to, step)?;
                let expressions = range
                    .map(|i| {
                        render_template(
                            &base_expression,
                            &HashMap::from_iter([(binding.as_str(), i.to_string())]),
                        )
                    })
                    .collect::<Vec<_>>();
                tracing::debug!(?binding, ?from, ?to, ?step, ?base_expression, ?expressions);

                let mut results = vec![];

                // All expressions are evaluated sequentially
                // (maybe in the future they could be evaluated concurrently, or with a --parallel flag)
                for expression in expressions {
                    let this = self.clone(); // fast clone

                    let result = this
                        .eval_job_without_channel(JobExpressionInner::String(expression.into()))
                        .with_tracy_non_continuous_frame("eval_job_for")
                        .await?;
                    results.push(result);
                }

                Ok(Variant::from(results))
            }

            MainCommand::Select {
                binding,
                expression,
            } => {
                // let's reserve => for :command new since it's more commonly used
                let parts: Vec<_> = expression.splitn(2, |x| x == "=->").collect();

                let [condition, body] = parts[..] else {
                    return Err(eyre!(
                        "expected exactly one =-> separator to separate condition from body"
                    ));
                };

                ensure!(
                    is_valid_variable_name(&binding),
                    "invalid binding name `{binding}`, must be alphanumeric, must not start with a digit, and cannot be a reserved keyword"
                );

                let condition: Arc<str> = Arc::from(condition.join(" "));
                let body: Arc<str> = Arc::from(body.join(" "));

                loop {
                    // TODO potential for infinite loop if result_body is not async - maybe wait one frame here? may mess up events though
                    // better idea: just do classify_future here
                    let result_body = self
                        .clone()
                        .eval_job_without_channel(JobExpressionInner::String(Arc::clone(&body)))
                        .with_tracy_non_continuous_frame("eval_job_select")
                        .await?;

                    self.gd_mut()
                        .bind_mut()
                        .script_context
                        .set_meta(&binding, &result_body);

                    let result_condition = self
                        .clone()
                        .eval_job_without_channel(JobExpressionInner::String(Arc::clone(
                            &condition,
                        )))
                        .with_tracy_non_continuous_frame("eval_job_select")
                        .await?;

                    // TODO instead use Variant::booleanize to make this more robust
                    if result_condition.try_to::<bool>().map_err(|_| {
                        eyre!(
                            "expected condition `{}` to return boolean, but it returned {:?}",
                            condition,
                            result_condition.get_type()
                        )
                    })? {
                        return Ok(Variant::from(result_body));
                    }
                }

                // Maybe unset the binding here? in defer?
                // Or not... I like being able to do this:
                // :repeat :seq :select i i.is_valid_int() =-> :key await |> print(i)
                // If you unset the binding, it would go away after the |>
            }

            MainCommand::Delta(delta) => delta.handle(self.clone()).await,
            MainCommand::Smooth(smooth) => smooth.handle(self.clone()).await,
            MainCommand::Plot(action) => action.handle(self.clone()).await,

            MainCommand::Log(action) => match action {
                LogAction::Level { level } => {
                    if let Some(level) = level {
                        set_log_level(&level);
                        Ok(Variant::nil())
                    } else {
                        Ok(match get_log_level() {
                            Some(level) => Variant::from(level),
                            None => Variant::nil(),
                        })
                    }
                }

                LogAction::Hook { enabled } => {
                    let mut log_hook = self
                        .bind_mut()
                        .log_hook
                        .as_mut()
                        .expect("log hook missing")
                        .clone(); // fast clone
                    if let Some(enabled) = enabled {
                        let new_val = match enabled {
                            BoolArg::Set(enabled) => log_hook.bind_mut().set_enabled(enabled),
                            BoolArg::Toggle => log_hook.bind_mut().toggle_enabled(),
                        };

                        Ok(Variant::from(new_val))
                    } else {
                        Ok(Variant::from(log_hook.bind().get_enabled()))
                    }
                }
            },

            MainCommand::Perf(action) => match action {
                PerfAction::Fps => Ok(Variant::from(Engine::singleton().get_frames_per_second())),

                PerfAction::Mem { bytes } => {
                    let mut ram = get_process_ram_bytes() as f64;

                    if !bytes {
                        ram /= 1024.0 * 1024.0;
                    }

                    Ok(Variant::from(ram))
                }

                PerfAction::Disk { mode, bytes } => {
                    let usage = get_process_total_disk_usage_bytes();
                    let (mut read, mut written) = (
                        usage.total_read_bytes as f64,
                        usage.total_written_bytes as f64,
                    );

                    if !bytes {
                        read /= 1024.0 * 1024.0;
                        written /= 1024.0 * 1024.0;
                    }

                    Ok(Variant::from(match mode {
                        DiskUsageMode::Read => read,
                        DiskUsageMode::Write => written,
                    }))
                }

                PerfAction::FrameTime { ms } => {
                    let mut frame_time = self.bind().last_frame_time.1;
                    if ms {
                        frame_time *= 1000.0;
                    }

                    Ok(Variant::from(frame_time))
                }
            },

            MainCommand::Profile(ProfileArgs { ms, expression }) => {
                let expression = expression.join(" ");

                let mut time = None;

                // Ignore expression result, so we can also measure a failing expression
                let _ = profile!(
                    "",
                    self.clone()
                        .eval_job_without_channel(JobExpressionInner::String(expression.into()))
                        .with_tracy_non_continuous_frame("eval_job_profile")
                        .await?,
                    |d: Duration| {
                        time = Some(if ms {
                            d.as_millis_f64()
                        } else {
                            d.as_secs_f64()
                        });
                    }
                );

                match time {
                    Some(time) => Ok(Variant::from(time)),
                    None => Err(eyre!(
                        "Failed to measure expression time - it probably panicked"
                    )),
                }
            }

            MainCommand::Sysinfo { pretty } => {
                let sysinfo = get_sysinfo();

                if pretty {
                    Ok(Variant::from(
                        Json::stringify_ex(&sysinfo.to_variant())
                            .indent("\t")
                            .sort_keys(false /* <-- important, or it will be reversed */)
                            .done(),
                    ))
                } else {
                    Ok(Variant::from(sysinfo))
                }
            }

            MainCommand::Debug(action) => match action {
                DebugAction::Allocate { size } => {
                    ensure!(size < 500 * 1024 * 1024, "buffer size must be <500MB");

                    let buffer = (0..size).map(|_| 0).collect::<Vec<u8>>();

                    // Now async sleep a bit to prevent the buffer from getting dropped immediately.
                    // let _guard = TOKIO_RUNTIME.enter(); // <-- don't do this - panics when you call :debug allocate concurrently
                    TOKIO_RUNTIME
                        .spawn(async {
                            // Thanks to TOKIO_RUNTIME.spawn we can safely use tokio here without panics
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        })
                        .await
                        .unwrap();

                    // This is interesting - even de-allocating the buffer causes a lag spike if it's big enough.
                    Ok(Variant::from(format!(
                        "Allocated {:.2} MB",
                        buffer.len() as f64 / 1024.0 / 1024.0
                    )))
                }
                DebugAction::Write { size } => {
                    // Doing this on the TOKIO_RUNTIME is significantly faster than just calling write_zeroes here directly.
                    // Why? Because we only poll the runtime 60x per second on the main thread.
                    // However, the tokio runtime gets polled much more often, so it makes way more progress per second.

                    ensure!(size < 5 * 1024 * 1024 * 1024, "file size must be <5GB");

                    let task = TOKIO_RUNTIME.spawn(async move { write_zeroes(size).await });
                    match task.await.expect("tokio task panicked") {
                        Ok(path) => Ok(Variant::from(format!(
                            "Wrote {:.2} MB to {path:?}",
                            size as f64 / 1024.0 / 1024.0
                        ))),
                        Err(err) => Err(err.into()),
                    }
                }

                DebugAction::HttpGet { url } => {
                    // Now using pure-Godot http requests, since reqwest is too big of a dependency.
                    // The `http` crate is only used to map HTTP status codes to their official names.
                    Ok(Variant::from(http_get(&url).await?))
                }

                DebugAction::Print {
                    pattern,
                    single_line,
                } => {
                    let node = if let Some(pattern) = pattern {
                        self.find_node_by_pattern(&pattern)?
                    } else {
                        self.gd().clone().upcast()
                    };

                    let dyn_node = node.try_dynify::<dyn ConsoleDebug>().map_err(|node| {
                        eyre!("found node `{node}` but it's either not a Rust class, or it doesn't impl the ConsoleDebug trait")
                    })?;

                    Ok(Variant::from(
                        dyn_node.dyn_bind().console_debug(!single_line),
                    ))
                }

                DebugAction::Panic => {
                    panic!("user requested panic");
                }

                DebugAction::Error => Err(eyre!("user requested error")),
            },

            MainCommand::Draw(action) => action.handle(self.clone()).await,
            MainCommand::EvalFile { filepath } => {
                // use TOKIO_RUNTIME.spawn here so we can use tokio's stuff
                let tokio_task = {
                    TOKIO_RUNTIME.spawn(async move {
                        // To check this safely: 1. canonicalize cwd 2. canonicalize file path 3. check if 2.starts_with(1)

                        // Note - in Godot, `cwd` is NOT the directory where you run `godot` from.
                        // Instead, it's the directory where `project.godot` is stored. (or PCK, if exported)
                        // So you can't bypass this by doing `cd /` and then running the game.
                        let cwd = tokio::fs::canonicalize(std::env::current_dir()?).await?;

                        let filepath_absolute =
                            polish_io_error(tokio::fs::canonicalize(&filepath).await, &filepath)?; // <- DOES access the filesystem
                        ensure!(
                            filepath_absolute.starts_with(&cwd),
                            "cannot evaluate file outside of the game's directory \n\
                            file path: {filepath_absolute:?} \n\
                            game directory: {cwd:?}"
                        );

                        let file = polish_io_error(
                            tokio::fs::File::open(&filepath_absolute).await,
                            &filepath_absolute,
                        )?;
                        let reader = tokio::io::BufReader::new(file);
                        let mut lines = reader.lines();
                        let mut line_vec = vec![];

                        while let Some(line) = lines.next_line().await? {
                            line_vec.push(line.trim().to_owned());
                        }

                        Ok::<_, Report>((line_vec, filepath_absolute))
                    })
                };

                // If any line fails to read, due to e.g. IO error or permission error, bail out immediately
                let (line_vec, filepath_absolute) =
                    tokio_task.await.expect("tokio task panicked")?;
                let line_vec_len = line_vec.len();

                for (i, line) in line_vec.into_iter().enumerate() {
                    tracing::info!(line, "evaluating from file ({i}/{})...", line_vec_len);
                    if line.is_empty() || line.starts_with("#") {
                        // no need to trim the line here, they're already trimmed
                        tracing::info!("skipping empty line/comment");
                        continue;
                    }
                    let _ = self
                        .clone()
                        .eval_job_without_channel(JobExpressionInner::String(line.into()))
                        .with_tracy_non_continuous_frame("eval_job_file")
                        .await?;
                }

                Ok(Variant::from(format!(
                    "Evaluated {} commands from file `{}`.",
                    line_vec_len,
                    filepath_absolute.to_string_lossy()
                )))
            }

            MainCommand::Cmd(action) => action.handle(self.clone()).await,

            MainCommand::Open { folder } => {
                let mut os = Os::singleton();

                let folder_name = match folder {
                    FolderType::Userdata => os.get_user_data_dir().to_string(), // Created by godot
                    FolderType::Logs => ProjectSettings::singleton()
                        .globalize_path("user://logs") // Created by godot
                        .to_string(),
                    FolderType::Screenshots => (*SCREENSHOTS_FOLDER
                        .with(|folder| (*folder).clone()))? // This can fail, since it will try to create the folder
                    .to_string_lossy()
                    .to_string(),
                };

                let err = os.shell_open(&folder_name);
                if err != GodotError::OK {
                    return Err(eyre!("Failed to open folder `{}`: {err:?}", folder_name));
                }

                Ok(Variant::from(format!("Opened folder `{}`.", folder_name)))
            }

            MainCommand::Dialog { expression } => {
                let expression = expression.join(" ");

                let expr_result = self
                    .clone()
                    .eval_job_without_channel(JobExpressionInner::String(expression.into()))
                    .with_tracy_non_continuous_frame("eval_job_dialog")
                    .await?;

                show_dialog("CrabbyConsole Dialog", &expr_result.stringify().to_string()).await;

                // Wait a little, otherwise if you call this in a loop it will spam this error:
                // _set_transient_exclusive_child: Attempting to make child window exclusive, but the parent window already has another exclusive child.
                wait_for_next_frame().await;

                // This is basically a shortcut for:
                // :seq :set d AcceptDialog.new() |> d.set_text("hi there") |> d.set_title("message") |> scene().add_child(d) |> d.popup_centered() |> d.visibility_changed |> d.queue_free()

                Ok(Variant::nil())
            }

            MainCommand::Prompt { expression } => {
                let expression = expression.join(" ");

                let expr_result = self
                    .clone()
                    .eval_job_without_channel(JobExpressionInner::String(expression.into()))
                    .with_tracy_non_continuous_frame("eval_job_prompt")
                    .await?;

                let result =
                    show_prompt("CrabbyConsole Prompt", &expr_result.stringify().to_string()).await;

                // Wait a little, otherwise if you call this in a loop it will spam this error:
                // _set_transient_exclusive_child: Attempting to make child window exclusive, but the parent window already has another exclusive child.
                wait_for_next_frame().await;

                Ok(if let Some(result) = result {
                    Variant::from(result)
                } else {
                    Variant::nil()
                })
            }

            MainCommand::Screenshot => {
                let path = take_screenshot().await?;

                Ok(Variant::from(format!(
                    "Screenshot saved to `{}`",
                    path.to_string_lossy() // You can run `:open userdata` to open that folder
                )))
            }
        }
    }
}

/// Looks up a key in the `build_key_name_map()` table.
/// This is case-insensitive by the way.
fn lookup_key(key: &str) -> eyre::Result<Key> {
    // Os.find_keycode_from_string() is not flexible enough w.r.t numbers and keypad

    // TODO maybe we can use Os::find_keycode_from_string instead of building a map.
    // That method also works with Shift+Tab and such. Could be neat for making shortcuts

    let keys = build_key_name_map();
    let Some(key_parsed) = keys.get(&key.to_lowercase()) else {
        return Err(eyre!(
            "invalid key {key:?}, to see a list of valid keys try `:key list --valid-keys`"
        ));
    };

    Ok(*key_parsed)
}

/// Check if the command has a marker T. Returns tuple (bool, full_command_chain)
fn check_marker<T: CommandExt>(cmd: &clap::Command, matches: &clap::ArgMatches) -> (bool, String) {
    let mut current_cmd = cmd;
    let mut current_matches = matches;
    let mut has_marker = current_cmd.get::<T>().is_some();

    let mut cmd_chain = current_cmd.get_name().to_string();

    // Descend down the tree of subcommands
    while let Some((name, sub_matches)) = current_matches.subcommand() {
        current_cmd = current_cmd
            .get_subcommands()
            .find(|s| s.get_name() == name)
            .expect("unreachable");

        cmd_chain.push_str(&format!(" {}", current_cmd.get_name()));

        if current_cmd.get::<T>().is_some() {
            has_marker = true; // we cannot break out of the loop here, or the command chain is too short
        }

        current_matches = sub_matches;
    }

    (has_marker, cmd_chain)
}

/// If the io error is "file not found", it adds the filepath to the error.
/// Otherwise, it may be unclear to the user where the file is expected to be found.
fn polish_io_error<T>(err: Result<T, std::io::Error>, filepath: &Path) -> Result<T, Report> {
    match err {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            Err(e).with_context(|| {
                // If this is a NotFound error, add the filepath to the error
                format!("file not found: {}", filepath.to_string_lossy())
            })
        }
        other => other.map_err(color_eyre::eyre::Error::from),
    }
}
