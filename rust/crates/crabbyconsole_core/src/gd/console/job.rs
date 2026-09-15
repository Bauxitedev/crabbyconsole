use std::{pin::Pin, rc::Rc, sync::Arc, time::Duration};

use color_eyre::eyre::Report;
use crabbyconsole_misc::{reflection::OpaqueDebug, util::is_fake_clap_error};
use godot::{
    classes::{RichTextLabel, Texture2D},
    prelude::*,
};
use mini_moka::unsync::Cache;

pub type JobExpressionCallableResult = Pin<Box<dyn Future<Output = Result<Variant, Report>>>>;

/// JobExpression must be Send, since it appears in `Job`, which is stored in a channel.
#[derive(Debug, Clone)] // Cloning a Callable seems to create another one that refers to the same function
pub enum JobExpression {
    String(Arc<str>), // Raw Gdscript/clap expression -> Using Arc<str> for cheap clones
    Callable(
        OpaqueDebug<
            // OpaqueDebug needed since closures don't impl Debug
            // Note - Arc<FnMut> will never work, since Arc innards are immutable
            // Also, this has to be an Arc, since we can create JobExpressions from another thread (e.g. remote console)
            Arc<dyn Send + Sync + Fn() -> JobExpressionCallableResult>,
        >,
    ), // Callable is !Send so store a closure instead
}

// Once evaluation starts, we convert JobExpression -> JobExpressionInner.
// So we go from Send -> !Send. This is safe, since evaluation always happens on the main thread.
#[derive(Debug, Clone)]
pub enum JobExpressionInner {
    String(Arc<str>), // TODO: this can be a Rc<> instead of Arc<>, since the other enum variant is also !Send
    Callable(OpaqueDebug<Rc<dyn Fn() -> JobExpressionCallableResult>>), // This is !Send
}

impl From<JobExpression> for JobExpressionInner {
    fn from(value: JobExpression) -> Self {
        match value {
            JobExpression::String(x) => JobExpressionInner::String(x),
            JobExpression::Callable(OpaqueDebug(arc)) => {
                // Turn Arc -> Rc
                let closure = move || (*arc)();
                JobExpressionInner::Callable(OpaqueDebug(Rc::new(closure)))
            }
        }
    }
}

/// Job is stored in a channel, so must be thread safe
pub(super) struct Job {
    pub(super) expression: JobExpression,
    pub(super) reply_tx: flume::Sender<JobEvent>, // Auxiliary channel to send information about the result back to the requester during the computation
    pub(super) use_timeout: bool,
}

/// Once expression evaluation starts, `Job` is converted to `JobInner`, which is no longer thread safe, so it can contain a Callable.
pub(super) struct JobInner {
    pub(super) expression: JobExpressionInner,
    pub(super) reply_tx: flume::Sender<JobEvent>,
    pub(super) use_timeout: bool,
}

impl From<Job> for JobInner {
    fn from(
        Job {
            expression,
            reply_tx,
            use_timeout,
        }: Job,
    ) -> Self {
        Self {
            expression: JobExpressionInner::from(expression),
            reply_tx,
            use_timeout,
        }
    }
}

#[derive(Debug)]
pub(crate) enum JobResultData {
    String(String),
    Image(InstanceId), // <-- must be thread safe, but ensure you use some kind of cache to keep the object alive (InstanceId = weakref)
    Color(Color),
}

#[derive(Debug)]
pub(crate) struct JobResultType {
    typ: VariantType,
    class: Option<String>, // If Type == OBJECT, this will store the class name
}

impl JobResultType {
    pub(super) fn fancy_type_name(&self) -> &str {
        if let Some(class) = &self.class {
            return class;
        }

        self.typ.godot_type_name()
    }
}

pub(super) struct JobResult {
    // We can't store Variant in `result`, since not thread-safe.
    // Also, we need Arc<JobError>, since eval_job sends the error to two different places, and eyre::Report can't be cloned regularly.
    pub(super) result: Result<(JobResultData, JobResultType), Arc<JobError>>,
    pub(super) execution_time: Duration,
    pub(super) async_wait_time: Option<Duration>, // Optional, only used if the result involved waiting for async
    pub(super) signal_wait_time: Option<Duration>, // Optional, only used if the result involved waiting for a signal
}

impl JobResultData {
    pub(super) fn prepare_job_result(
        eval_result: &Result<Variant, Arc<JobError>>,
        cache: &mut Cache<InstanceId, Gd<Texture2D>>,
    ) -> Result<(Self, JobResultType), Arc<JobError>> {
        eval_result
            .as_ref()
            .map(|var| {
                let class_name = {
                    if let Ok(obj) = var.try_to::<Gd<Object>>() {
                        Some(obj.get_class().to_string()) // Use Rust string here because it's thread safe
                    } else {
                        None
                    }
                };
                (
                    Self::new(var, cache),
                    JobResultType {
                        typ: var.get_type(),
                        class: class_name,
                    },
                )
            })
            .map_err(Arc::clone)
    }
}

pub(super) enum JobEvent {
    WaitingAsync,    // Indicate to user/watch we're waiting for an async call to complete
    WaitingSignal,   // Indicate to user/watch we're waiting for a signal to complete
    Done(JobResult), // The job finished, send the final result to the user
}

impl JobResultData {
    pub(super) fn add_to(
        &self,
        mut rt: Gd<RichTextLabel>,
        cache: &mut Cache<InstanceId, Gd<Texture2D>>,
    ) {
        match self {
            JobResultData::String(string) => rt.add_text(string), // ignores bbcode
            JobResultData::Image(id) => {
                // This doesn't work, since instance get freed too early
                // let img = Gd::<Texture2D>::try_from_instance_id(*id);
                // Instead, use a cache:
                let img = cache.get(id).cloned();

                if let Some(img) = img {
                    let img_h = img.get_height();
                    let max_h = 200;
                    let img_str = &format!("{img:?}");

                    rt.add_image_ex(&img)
                        .height(img_h.min(max_h))
                        .tooltip(img_str) // tooltip only works in the console, not in the watch panel
                        .color(Color::from_rgba(1., 1., 1., 0.8)) // slightly transparent
                        .done(); // don't make the image taller than it needs to be
                } else {
                    // Cache miss
                    rt.add_text(&format!(
                        "<texture {id} was destroyed before we had the opportunity to draw it>"
                    ));
                }
            }
            JobResultData::Color(color) => {
                let color_stringified = color.to_string();
                rt.add_text(&color_stringified); // ignores bbcode
                rt.push_color(*color);
                rt.add_text(" █"); // visualize the color by drawing a little colored block next to it
                rt.pop();
            }
        }
    }
}

impl JobResultData {
    fn new(value: &Variant, cache: &mut Cache<InstanceId, Gd<Texture2D>>) -> Self {
        // Always try to cast to the most specific type first!
        if let Ok(tex) = value.try_to::<Gd<Texture2D>>() {
            let id = tex.instance_id();

            // Put the texture in the cache, so it doesn't immediately get destroyed
            cache.insert(id, tex);

            // Return its instance id
            JobResultData::Image(id)
        } else if let Ok(color) = value.try_to::<Color>() {
            JobResultData::Color(color)
        } else {
            JobResultData::String(value.stringify().to_string())
        }
    }
}

// Errors that can occur while evaluating an expression.
#[derive(thiserror::Error, Debug)]
pub enum JobError {
    // TODO maybe add parse error or something, add any error that would be useful to show in the watch window
    #[error("async call timed out after {0:?} - to increase timeout, run `:async timeout 60`.")]
    TimeoutAsync(Duration),
    #[error("signal timed out after {0:?} - to increase timeout, run `:async timeout 60`.")]
    TimeoutSignal(Duration),
    #[error(transparent)]
    EvalGodot(#[from] EvalGodotError),
    #[error(transparent)]
    EvalClap(#[from] EvalClapError),
}

/// Errors that can occur while evaluating a Godot expression.
/// It seems detecting runtime errors in `GDScript` is impossible, sadly.
/// So we only get compile errors here, and some other misc edge cases.
#[derive(thiserror::Error, Debug)]
pub enum EvalGodotError {
    #[error("compile error: {0:?}")]
    CompileError(godot::global::Error),

    // I've never managed to trigger this error naturally yet
    // You can trigger it manually using DEBUG_INVALID_CALL
    // It will show an error like:
    //      Error: call error: godot-rust function call failed: Object::call(&"invalid_call")
    //      Reason: method not found
    #[error("call error: {0}")]
    CallError(#[from] godot::meta::error::CallError),

    #[error("GDScript execution disallowed, since lockdown mode is enabled")]
    Lockdown,

    #[error(transparent)]
    Other(#[from] Report), // Misc error e.g. "Empty expression"
}

/// Errors that can occur while evaluating a Clap expression.
#[derive(thiserror::Error, Debug)]
pub enum EvalClapError {
    #[error(transparent)]
    ClapHelp(clap::error::Error), // actually a "fake" error

    #[error(transparent)]
    ClapOther(clap::error::Error),

    #[error("Running risky command `{risky_cmd}` is disallowed, since lockdown mode is enabled")]
    Lockdown { risky_cmd: String },

    #[error(transparent)]
    Other(#[from] Report), // error from the body of handle_clap_command
}

// Impl From manually instead of using #[from], so we can distinguish between fake errors and real ones.
impl From<clap::error::Error> for EvalClapError {
    fn from(err: clap::error::Error) -> Self {
        if is_fake_clap_error(&err) {
            EvalClapError::ClapHelp(err)
        } else {
            EvalClapError::ClapOther(err)
        }
    }
}
