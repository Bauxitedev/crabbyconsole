pub mod context;
pub mod custom;
pub mod delta;
pub mod draw;
pub mod eval_clap;
pub mod node;
pub mod plot;
pub mod plot_draw;
pub mod singletons;
pub mod smooth;
pub mod tween;
pub mod vr;
pub mod watch;

use color_eyre::{
    Result as EyreResult,
    eyre::{ContextCompat as _, OptionExt as _, Report, eyre},
};
use crabbyconsole_clap::{clap_util::PositionArg, eval_definition::CLAP_COMMAND_PREFIX};
#[cfg(feature = "tracy")]
use crabbyconsole_misc::tracy::get_tracy_client_or_panic;
use crabbyconsole_misc::{
    flags::ADV_GODOT_EXPR_EVAL_PRINT_SRC_CODE_FLAG,
    gd::async_node::AsyncGd,
    reflection::OpaqueDebug,
    util::{get_camera_2d, get_camera_3d, get_viewport},
};
use godot::{classes::*, global::Error, prelude::*};
use rand::RngExt as _;
use tap::Tap as _;

use crate::gd::console::{
    CrabConsole,
    eval::context::CrabConsoleScriptContext,
    job::{EvalGodotError, JobError, JobExpressionCallableResult, JobExpressionInner},
    lockdown::is_lockdown_enabled,
    util::get_cursor_world_position,
};

/// Little helper trait so we can start moving code out of the behemoth called `eval_clap.rs`
pub(super) trait ClapSubAction {
    async fn handle(self, console: AsyncGd<CrabConsole>) -> Result<Variant, Report>;
}

impl CrabConsole {
    #[tracing::instrument(skip_all)]
    pub async fn eval(self: AsyncGd<Self>, code: JobExpressionInner) -> Result<Variant, JobError> {
        {
            match code {
                JobExpressionInner::String(code) => {
                    // Filter all control chars here and newlines
                    // Do this here since using eval_blocking you can send arbitrary bad things into this method.
                    let code: String = code
                        .chars()
                        .map(|c| if c == '\t' { ' ' } else { c }) // tab becomes space
                        .filter(|c| !c.is_control())
                        .collect();
                    //TODO doesn't work for ZWJ (zero width joiner)
                    //TODO maybe trim the start of the string as well?
                    //TODO use a newtype SanitizedString in all descendent methods to encode this information in the type system

                    #[cfg(feature = "tracy")]
                    get_tracy_client_or_panic().message(&format!("Evaluating: {code}"), 60); // tracing::info causes massive delay here

                    // TODO this tracing call may be slow/cause crashes in mem_test?
                    tracing::trace!(code, "Evaluating...");

                    // If code starts with :, evaluate the rest with clap instead of Godot's Expression.
                    if let Some(clap) = code.strip_prefix(CLAP_COMMAND_PREFIX) {
                        self.eval_clap(clap).await.map_err(JobError::EvalClap)
                    } else {
                        // This is not async - if it returns a signal, it will be awaited later
                        self.eval_godot_advanced(&code).map_err(JobError::EvalGodot)
                    }
                }
                JobExpressionInner::Callable(OpaqueDebug(callable)) => self
                    .eval_callable(|| callable()) // <-- this turns the `Rc<dyn Fn() -> ...>` into `impl Fn() -> ...`
                    .await
                    .map_err(JobError::EvalGodot),
            }
        }
        .tap(|result| {
            #[cfg(feature = "tracy")]
            {
                use crabbyconsole_misc::reflection::SafeDebug as _;
                let result_str = result.safe_debug(); // <-- Safe now, even if result is a Callable or GDScriptNativeObject

                // Warning: limit to 65k chars max, otherwise Tracy hard-crashes your program!
                let too_large = result_str.len() >= (u16::MAX as usize);
                let msg = format!(
                    "Evaluated to: {}",
                    if too_large {
                        format!(
                            "<evaluation result too large
    (was {} characters, but the limit is {})>",
                            result_str.len(),
                            u16::MAX
                        )
                    } else {
                        result_str
                    }
                );

                get_tracy_client_or_panic().message(&msg, 60);
            }

            // Use the variable when `tracy` is disabled, to avoid unused variable warning.
            #[cfg(not(feature = "tracy"))]
            let _ = result;
        })
    }

    #[tracing::instrument(skip_all)]
    pub async fn eval_callable(
        self: AsyncGd<Self>,
        callable: impl Fn() -> JobExpressionCallableResult,
    ) -> Result<Variant, EvalGodotError> {
        let result = callable().await?;

        Ok(result)
    }

    /// Evaluate the expression using the advanced evaluator.
    /// Note - this needs to take a `AsyncGd`, even though we're not async, to ensure self is NOT borrowed during the execution of the expression.
    /// Otherwise, it will panic if you try to access anything on `self` within the expression, due to double-borrow.
    ///
    /// Unlike the simple evaluator, the advanced evaluator supports assignment, closures, the ternary operator, and the $/% syntax.
    ///
    /// Update - this is now v3 of the evaluator (v2 was the advanced evaluator, v1 was the simple one).
    /// This one uses `ScriptContext`, which has the following advantages:
    /// 1. no longer crashes in exported build
    /// 2. memory leak is essentially gone -> but not entirely gone
    ///
    /// WARNING: If `code` contains newlines, it may be possible to break out of the syntax and produce pathological cases and cause bad things to happen.
    /// So filtering that now in `eval()`.
    #[tracing::instrument(skip_all)]
    pub fn eval_godot_advanced(
        mut self: AsyncGd<Self>,
        code: &str,
    ) -> Result<Variant, EvalGodotError> {
        {
            //debug stuff, maybe turn this into an integration test instead? this is ugly
            const DEBUG_INVALID_CALL: bool = false;

            if DEBUG_INVALID_CALL {
                return Ok(self.gd_mut().try_call("invalid_call", &[])?);
            }
        }

        if is_lockdown_enabled() {
            // In lockdown mode, GDScript is not allowed to be evaluated.
            return Err(EvalGodotError::Lockdown);
        };

        if code.contains("\n") {
            // TODO add a type-level newtype for this, so we don't have to check this manually anymore.
            panic!("Code contains newlines - this should never be allowed to happen");
        }

        // Split on `;`, trim each line, re-join with newline + tab
        let lines: Vec<&str> = code
            .split(';')
            .map(|line| line.trim_start())
            .filter(|line| !line.is_empty())
            .collect();

        // All but the last line are statements - last line is attempted as expression
        let (last, rest) = lines.split_last().ok_or(eyre!("Empty expression"))?;
        let body_prefix = rest.join("\n\t");

        // Add warning ignore attributes to the run() method, otherwise Godot get very spammy about it.
        // See https://github.com/godotengine/godot/pull/76020
        // And https://docs.godotengine.org/en/stable/classes/class_%40gdscript.html#class-gdscript-annotation-warning-ignore-start
        let attributes = [
            r#"@warning_ignore_start("unused_variable")"#,
            r#"@warning_ignore_start("unused_parameter")"#,
            r#"@warning_ignore_start("return_value_discarded")"#,
            r#"@warning_ignore_start("shadowed_variable")"#,
            r#"@warning_ignore_start("integer_division")"#,
        ]
        .iter()
        .map(|s| format!("\t{s}")) // All of these need a \t at the start
        .collect::<Vec<_>>()
        .join("\n");

        // Turns every meta definition into a statement like `var foo = get_meta("foo")`
        // This means we can evaluate expressions like `x*2 + y` instead of `get_meta("x")*2 + get_meta("y")`
        // This seems safe, because Godot checks if the name you pass to set_meta() has no special characters like \n / , . - " ' ^ 0 space etc
        // TODO ensure you can't store meta names with invalid characters in a scene and then load that though...
        let metas = self
            .gd()
            .bind()
            .script_context
            .get_meta_list()
            .iter_shared()
            .map(|meta_name| format!(r#"var {meta_name} = get_meta("{meta_name}")"#))
            .collect::<Vec<_>>()
            .join("\n\t");
        // TODO for loops/ifs with a body with more than 1 line don't work

        // Generate a unique random function name, to prevent creating a stack overflow when you type `run()` in the console.
        let mut rng = rand::rng();
        let function_name = format!("run_{}", rng.random::<u64>());

        let gen_source_code = |body: String| {
            let class = CrabConsoleScriptContext::class_id();
            let result = format!(
                "extends {class}\n\nfunc {function_name}():\n{attributes}\n\t{metas}\n\t{body}\n"
            );

            result.tap(|source_code| {
                if ADV_GODOT_EXPR_EVAL_PRINT_SRC_CODE_FLAG.get() {
                    tracing::info!(%source_code) // % means "use Display impl" so prints newlines as-is
                }
            })
        };

        // Try last line as expression first (with return)
        let expr_body = if body_prefix.is_empty() {
            format!("return {}", last)
        } else {
            format!("{}\n\treturn {}", body_prefix, last)
        };

        // To prevent memory leak, re-use the original script instead of creating a new one.
        // See https://github.com/godotengine/godot/issues/99169#issuecomment-2484976974
        let mut script = self.bind().master_script.clone(); // fast clone
        // Note: duplicate_resource() also leaks memory, so don't use it.

        script.set_source_code(&gen_source_code(expr_body));

        // If that fails, fall back to treating last line as a statement too
        if script.reload() != Error::OK {
            let stmt_body = if body_prefix.is_empty() {
                last.to_string()
            } else {
                format!("{}\n\t{}", body_prefix, last)
            };

            script.set_source_code(&gen_source_code(stmt_body));

            let err = script.reload();

            if err != Error::OK {
                // Both "return expr" and "expr" failed to compile, so bail out
                //tracing::warn!("script failed",);

                return Err(EvalGodotError::CompileError(err));
            }
        }

        let mut script_context = self.bind().script_context.clone(); // clone to avoid holding bind during try_call
        script_context.set_script(&script);

        // This failed in expression evaluator v2, fixed in v3:
        let ran = script_context.try_call(&function_name, &[])?; // This should NEVER fail
        script_context.set_script(None::<&Gd<Script>>); // <-- Maybe do this with defer? I tried to panic in various ways but it seems to work fine though, it doesn't remain in a bad state

        Ok(ran)
    }

    // These are separate methods, to avoid type erasure due to Variant.

    pub(super) fn get_root(self: &AsyncGd<Self>) -> Gd<Window> {
        self.gd().get_tree().get_root().expect("no root node")
    }

    pub(super) fn find_nodes_variant(self: &AsyncGd<Self>, pattern: &str) -> Variant {
        Variant::from(
            self.find_nodes_by_pattern(pattern)
                .into_iter()
                .map(Variant::from)
                .collect::<Vec<_>>(),
        )
    }

    pub(super) fn find_node_by_pattern(
        self: &AsyncGd<Self>,
        pattern: &str,
    ) -> EyreResult<Gd<Node>> {
        // Let's just always use find_nodes, since it has more functionality anyway

        self.find_nodes_by_pattern(pattern)
            .into_iter()
            .next()
            .with_context(|| format!("no nodes found with given pattern `{pattern}`"))
    }

    pub(super) fn find_nodes_by_pattern(self: &AsyncGd<Self>, pattern: &str) -> Vec<Gd<Node>> {
        let root = self.get_root();

        root.find_children_ex(pattern)
            .owned(false) // Note - by default, find_children only returns nodes that have a valid `owner` set.
            .done()
            .iter_shared()
            .collect::<Vec<_>>()
    }

    /// Finds the first node whose path contains `nodepath_needle`. Useful if nodes have duplicate names.
    pub(super) fn find_node_by_nodepath_needle(
        self: &AsyncGd<Self>,
        nodepath_needle: &str,
        typ: Option<&str>,
    ) -> EyreResult<Gd<Node>> {
        // Note - in the future, you may want to do more advanced sorting to ensure we return perfect matches first.
        // E.g. right now, if you search for `/root/`, you will NOT get the root node, which is problematic.

        self.find_nodes_by_nodepath_needle(nodepath_needle, typ)
            .into_iter()
            .next()
            .ok_or_else(|| {
                let type_hint = if let Some(typ) = typ {
                    format!("(and type `{typ}`)")
                } else {
                    String::new()
                };
                eyre!("no node found with given NodePath needle: `{nodepath_needle}` {type_hint}")
            })
    }

    /// Finds all nodes whose path contains `nodepath_needle`, optionally filtered by type `typ`.
    ///
    /// Useful if nodes have duplicate names.
    ///
    /// If `nodepath_needle` == "", it will return all nodes in the scene (optionally filtered by type `typ`),
    /// because `node.get_path().to_string().contains("")` is always `true`.
    pub(super) fn find_nodes_by_nodepath_needle(
        self: &AsyncGd<Self>,
        nodepath_needle: &str,
        typ: Option<&str>,
    ) -> Vec<Gd<Node>> {
        let root = self.get_root();

        // Only prepend the root node if its type matches.
        let prepend_root_node: bool = typ.is_none_or(|typ| root.is_class(typ));

        prepend_root_node
            .then_some(root.clone().upcast())
            .into_iter() // Empty iterator if None, else [Gd<Node>]
            .chain(
                root.find_children_ex("*")
                    .owned(false)
                    .type_(typ.unwrap_or("")) // empty type = all types allowed
                    .done()
                    .iter_shared(),
            )
            .filter(|n| n.get_path().to_string().contains(nodepath_needle))
            .collect()
    }

    /// Resolves a [`PositionArg`] to a world-space [`Vector2`].
    pub fn resolve_position_2d(
        self: &AsyncGd<Self>,
        position: &PositionArg,
    ) -> EyreResult<Vector2> {
        match position {
            // TODO add PositionArg::Variable(string)?
            PositionArg::Coords2D(xy) => Ok(*xy),
            PositionArg::Coords3D(xyz) => {
                Err(eyre!("expected a 2D position, but got a 3D one ({xyz})"))
            }
            PositionArg::Cursor => Ok(get_viewport().get_mouse_position()), // See ScriptExecutionContext::cursor_2d
            PositionArg::Camera => Ok(get_camera_2d()
                .ok_or_eyre("no Camera2D in scene")?
                .get_global_position()),
            PositionArg::Node(pattern) => self
                .find_node_by_pattern(pattern)
                .and_then(|node| {
                    node.try_cast::<Node2D>()
                        .map(Resolvable2D::from)
                        .or_else(|node| node.try_cast::<Control>().map(Resolvable2D::from))
                        .map_err(|node| {
                            eyre!("found node `{node}` but it wasn't a Control or Node2D")
                        })
                })
                .map(|node| node.get_global_position()),
        }
    }

    /// Resolves a [`PositionArg`] to a world-space [`Vector3`].
    pub fn resolve_position_3d(
        self: &AsyncGd<Self>,
        position: &PositionArg,
    ) -> EyreResult<Vector3> {
        // Do this lazily
        let cam3d = || get_camera_3d().ok_or_eyre("no Camera3D in scene");

        match position {
            PositionArg::Coords2D(xy) => {
                Err(eyre!("expected a 3D position, but got a 2D one ({xy})"))
            }
            PositionArg::Coords3D(xyz) => Ok(*xyz),
            PositionArg::Cursor => Ok(get_cursor_world_position(3.0, cam3d()?)),
            PositionArg::Camera => Ok(cam3d()?.get_global_position()),
            PositionArg::Node(pattern) => self
                .find_node_by_pattern(pattern)
                .and_then(|node| {
                    node.try_cast::<Node3D>()
                        .map_err(|node| eyre!("found node `{node}` but it wasn't a Node3D"))
                })
                .map(|node| node.get_global_position()),
        }
    }

    pub(super) fn find_by_pattern_infallible(
        self: &AsyncGd<Self>,
        pattern: &str,
        all: bool,
    ) -> Vec<Gd<Node>> {
        let mut targets = vec![];
        if all {
            targets.append(&mut self.find_nodes_by_pattern(pattern));
        } else if let Ok(x) = self.find_node_by_pattern(pattern) {
            targets.push(x);
        };
        targets
    }

    pub(super) fn find_by_nodepath_needle_infallible(
        self: &AsyncGd<Self>,
        pattern: &str,
        typ: Option<&str>,
        all: bool,
    ) -> Vec<Gd<Node>> {
        let mut targets = vec![];
        if all {
            targets.append(&mut self.find_nodes_by_nodepath_needle(pattern, typ));
        } else if let Ok(x) = self.find_node_by_nodepath_needle(pattern, typ) {
            targets.push(x);
        };
        targets
    }
}

/// Little helper enum for `resolve_position_2d`
enum Resolvable2D {
    Node2D(Gd<Node2D>),
    Control(Gd<Control>),
}

impl Resolvable2D {
    fn get_global_position(&self) -> Vector2 {
        match self {
            Resolvable2D::Node2D(gd) => gd.get_global_position(),
            Resolvable2D::Control(gd) => gd.get_global_position(),
        }
    }
}

impl From<Gd<Node2D>> for Resolvable2D {
    fn from(value: Gd<Node2D>) -> Self {
        Resolvable2D::Node2D(value)
    }
}

impl From<Gd<Control>> for Resolvable2D {
    fn from(value: Gd<Control>) -> Self {
        Resolvable2D::Control(value)
    }
}
