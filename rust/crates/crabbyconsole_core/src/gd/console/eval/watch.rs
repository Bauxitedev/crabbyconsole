use std::{cell::RefCell, rc::Rc, sync::Arc, time::Instant};

use color_eyre::eyre::{Report, ensure, eyre};
use crabbyconsole_clap::eval_definition::{WatchAction, WatchNameStyle};
use crabbyconsole_misc::{
    FutureTracyExt as _,
    gd::async_node::{AsyncGd, AsyncNode as _},
    reflection::OpaqueDebug,
    util::get_root,
};
use godot::prelude::*;
use indexmap::IndexMap;
use scopeguard::defer;

use crate::gd::console::{
    CrabConsole,
    eval::ClapSubAction,
    job::{
        JobError, JobEvent, JobExpressionCallableResult, JobExpressionInner, JobInner,
        JobResultData, JobResultType,
    },
    util::is_valid_variable_name,
};

#[derive(Debug, Default)]
pub(crate) struct Watches {
    pub(crate) watches: OpaqueDebug<IndexMap<String, Watch>>,
    pub(crate) paused: bool,
}

#[derive(Debug, Default)]
pub(crate) enum WatchSlotKind {
    #[default]
    Sync,
    Async,
    Signal,
}

// TODO this seems remarkably similar to JobResult...
#[derive(Debug)]
pub(crate) struct WatchResult {
    pub(crate) result_stringified: Result<(JobResultData, JobResultType), Arc<JobError>>,
    pub(crate) timestamp: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct WatchSlot {
    pub(crate) is_busy: bool, // default false
    pub(crate) kind: WatchSlotKind,
    pub(crate) result: Option<WatchResult>,
    pub(crate) last_evaluation_time: Option<Instant>, // timestamp storing the date/time of the last time evaluation was started
}

#[derive(Debug, Clone)]
pub(crate) enum WatchFrequency {
    /// Evaluate every frame
    EveryFrame,

    /// Evaluate at most x times per second (bounded by frame rate).
    Rate(f64),
}

#[derive(Debug)]
pub(crate) struct Watch {
    pub(crate) expression: JobExpressionInner,
    pub(crate) frequency: WatchFrequency, // How many times to evaluate the expression per second
    pub(crate) slot: Rc<RefCell<WatchSlot>>, // stores the result/status of the calculation of the expression
}

impl Watch {
    pub(super) fn clone_with_empty_slot(&self) -> Watch {
        let Watch {
            expression,
            frequency,
            ..
        } = self;
        Watch {
            expression: expression.clone(),
            frequency: frequency.clone(),
            slot: Rc::new(RefCell::new(WatchSlot::default())),
        }
    }
}

impl CrabConsole {
    #[tracing::instrument(skip_all)]
    pub(crate) fn run_pending_watch_jobs(&mut self) {
        let task = self.bound_task();

        if self.watches.paused {
            // watches are paused, so do nothing
            // note: pending calculations/signals from previous frames can still arrive though, since they run on background tasks
            return;
        }

        for (
            _,
            Watch {
                expression,
                frequency,
                slot,
            },
        ) in &*self.watches.watches
        {
            if slot.borrow().is_busy {
                // Already calculating, so skip it
                continue;
            }

            if let WatchFrequency::Rate(rate) = frequency
                && let Some(last) = slot.borrow().last_evaluation_time
                && last.elapsed().as_secs_f64() < 1.0 / *rate
            {
                // The watch is in rate-limited mode and the time since last evaluation was too short, so skip it
                // (Note - `:watch add` ensures rate > 0, so we can't have division by zero here)
                continue;
            }

            let (reply_tx, reply_rx) = flume::unbounded();
            let job = JobInner {
                expression: expression.clone(),
                reply_tx,
                use_timeout: false, // watch jobs have no timeout
            };

            let start_time = Instant::now();
            slot.borrow_mut().last_evaluation_time = Some(start_time);

            {
                let slot = Rc::clone(slot);
                task.new(async move |mut this| {
                    // `is_busy` is a Rc<Cell<bool>> so it's guaranteed to ONLY be modified from us.
                    // Every task gets their own copy of it.
                    slot.borrow_mut().is_busy = true;

                    defer! {
                        slot.borrow_mut().is_busy = false;
                    }

                    let job_result = this
                        .clone() // fast clone
                        .eval_job(job)
                        .with_tracy_non_continuous_frame("eval_job_watch")
                        .await;

                    // Note - this overrides the result only if we've NOT been interrupted.
                    // so you swap it out for a new RefCell, so mutating the stale one won't cause noticeable results in the ui.
                    slot.borrow_mut().result.replace(WatchResult {
                        result_stringified: JobResultData::prepare_job_result(
                            &job_result,
                            &mut this.bind_mut().job_result_cache,
                        ),
                        timestamp: Instant::now(),
                    });

                    // defer! sets `busy` to false here.
                })
                .spawn();
            }

            // We need a separate task here, since doing eval_job().await means we cannot receive any events until the job is fully completed.
            let slot = Rc::clone(slot);
            task.new(async move |_| {
                while let Ok(e) = reply_rx.recv_async().await {
                    match e {
                        // Update watch slot kind depending on the events that come in.
                        // Note that the type of a watch can change while its executing.
                        // E.g. an async function could return a signal after one second.
                        // So the first second it will be Async, then Signal.
                        // Then, when it gets re-evaluated, it becomes Async again.
                        JobEvent::WaitingAsync => slot.borrow_mut().kind = WatchSlotKind::Async,
                        JobEvent::WaitingSignal => slot.borrow_mut().kind = WatchSlotKind::Signal,
                        _ => {}
                    };
                }
                // At this point reply_rx is dropped.
            })
            .spawn();
        }
    }
}

impl ClapSubAction for WatchAction {
    async fn handle(self, mut console: AsyncGd<CrabConsole>) -> Result<Variant, Report> {
        match self {
            WatchAction::Add {
                name,
                expression,
                rate,
            } => {
                let expression = expression.join(" ");

                // TODO maybe check this using clap's value-parser instead!
                let frequency = if let Some(rate) = rate {
                    ensure!(rate > 0.0, "`rate` must be greater than 0");
                    ensure!(rate.is_finite(), "`rate` must be finite");
                    WatchFrequency::Rate(rate)
                } else {
                    WatchFrequency::EveryFrame
                };

                // If watch is already present, overrides it
                let prev = console.bind_mut().watches.watches.insert(
                    name.clone(),
                    // We will determine later if this is a sync, async, or signal watch.
                    // Right now we don't have enough information for that yet...
                    // ...we need to wait for events to be sent on the result channel to change its type to the correct one during execution.
                    Watch {
                        expression: JobExpressionInner::String(expression.into()),
                        frequency,
                        slot: Rc::new(RefCell::new(WatchSlot::default())),
                    },
                );

                Ok(Variant::from(if prev.is_some() {
                    format!("Overwrote watch `{name}`.")
                } else {
                    format!("Added new watch `{name}`.")
                }))
            }
            WatchAction::Remove { prefix, all } => {
                let watches = &mut console.bind_mut().watches.watches;
                if all {
                    // Find all matches
                    let mut removed = Vec::new();

                    watches.retain(|key, _| {
                        if key.starts_with(&prefix) {
                            removed.push(key.clone());
                            false // drop it
                        } else {
                            true // keep it
                        }
                    });

                    Ok(Variant::from(if !removed.is_empty() {
                        format!("Removed {} watches:\n{}", removed.len(), removed.join("\n"))
                    } else {
                        format!("No watches found with prefix `{prefix}`.")
                    }))
                } else {
                    // Find 1 match
                    let prev = if let Some(key) =
                        watches.keys().find(|key| key.starts_with(&prefix)).cloned()
                    {
                        watches.shift_remove(&key);
                        Some(key)
                    } else {
                        None
                    };

                    Ok(Variant::from(if let Some(prev) = prev {
                        format!("Removed watch `{prev}`.")
                    } else {
                        format!("No watch found with prefix `{prefix}`.")
                    }))
                }
            }
            WatchAction::List => {
                let list = console
                    .bind_mut()
                    .watches
                    .watches
                    .iter()
                    .map(|(name, Watch { expression, .. })| format!("{name} = {expression:?}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(Variant::from(list))
            }
            WatchAction::Clear => {
                let len = console.bind().watches.watches.len();
                console.bind_mut().watches.watches.clear();

                Ok(Variant::from(format!("Cleared {len} watches.")))
            }
            WatchAction::Refresh => {
                // Re-create all watches from scratch
                for v in console.bind_mut().watches.watches.values_mut() {
                    *v = v.clone_with_empty_slot();
                }

                Ok(Variant::from(format!(
                    "Refreshed {} watches.",
                    console.bind().watches.watches.len()
                )))
            }
            WatchAction::Pause => {
                console.bind_mut().watches.paused ^= true; // toggle
                Ok(Variant::from(console.bind().watches.paused))
            }
            WatchAction::Signals {
                expression,
                use_instance_id,
                style,
            } => {
                let expression = expression.join(" ");

                let candidate = console
                    .clone()
                    .eval_job_without_channel(JobExpressionInner::String(expression.into()))
                    .with_tracy_non_continuous_frame("eval_job_watch_signals")
                    .await?;

                let candidate = if let Ok(node) = candidate.try_to::<Gd<Node>>() {
                    // Try to cast to Node first, because Node is more specific than Object.
                    Watchable::Node(node)
                } else if let Ok(res) = candidate.try_to::<Gd<Resource>>() {
                    // Then try to cast to Resource
                    Watchable::Resource(res)
                } else if let Ok(obj) = candidate.try_to::<Gd<Object>>() {
                    // Finally try Object
                    // (Things like SceneTree and Input (+ all singletons) are all Objects)
                    Watchable::Object(obj)
                } else {
                    return Err(eyre!(
                        "Expected an expression of type Node, Resource or Object, but instead got type {:?}",
                        candidate.get_type()
                    ));
                };

                // This is just the class name for Objects - it can be "" if it's a Resource, so added a fallback for that in get_name()
                let object_name = candidate.get_name();
                let signals = candidate.get_inner().get_signal_list();
                let path = candidate.get_path();

                let watch_name_prefix = match style {
                    WatchNameStyle::Name => object_name.to_string(),
                    WatchNameStyle::NameWithId => format!("{}", candidate.get_inner()),
                    WatchNameStyle::Path => {
                        if let Some(path) = &path {
                            format!("{path}")
                        } else {
                            return Err(eyre!(
                                "User requested to visualize watch using the `Path` style, \
                                    but the given expression did not return a Node or Resource, \
                                    so it does not have a path"
                            ));
                        }
                    }
                };

                for signal in signals.iter_shared() {
                    let signal_name = signal
                        .get("name")
                        .expect("signal has no name") // this should never panic unless Godot changes its API
                        .to::<String>();

                    if !is_valid_variable_name(&signal_name) {
                        // Update - now using safe Callable, so the following information may no longer be correct.
                        // Can't inject GDScript anymore! But, it's probably still a good idea to use sane variable names, so keeping the check.

                        // [old] This check is very important, otherwise you can splice arbitrary GDScript into the watch evaluator and cause bad things to happen.
                        // Godot does not sanitize the signal name in Node.add_user_signal(signal_name) so it can contain all kinds of nasty stuff!
                        // On the other hand, the NodePath should be safe, because Godot does sanitize it. [/old]

                        tracing::warn!(signal_name, "invalid signal name, skipping it");
                        continue;
                    }

                    let watch_name = format!("{watch_name_prefix}.{signal_name}");
                    let callable = make_signal_callable(
                        signal_name,
                        candidate.get_inner(),
                        path.clone(),
                        use_instance_id,
                    );

                    console.bind_mut().watches.watches.insert(
                        watch_name,
                        Watch {
                            expression: JobExpressionInner::Callable(OpaqueDebug(callable)),
                            frequency: WatchFrequency::EveryFrame,
                            slot: Rc::new(RefCell::new(WatchSlot::default())),
                        },
                    );
                }

                let user_facing_name = if let Some(path) = path {
                    path.to_string()
                } else {
                    candidate.get_inner().to_string()
                };

                Ok(Variant::from(format!(
                    "Added {} watches for {} `{}`.",
                    signals.len(), // <-- TODO this length is no longer accurate, since we filter watches now
                    candidate.get_kind(),
                    user_facing_name
                )))
            }
        }
    }
}

/// Creates a closure that returns the Signal object corresponding to the given `signal_name` and `inner_obj`.
fn make_signal_callable(
    signal_name: String,
    obj: Gd<Object>,
    path: Option<WatchablePath>,
    use_instance_id: bool,
) -> Rc<dyn Fn() -> JobExpressionCallableResult> {
    // Looking nodes up by ID should be faster than by NodePath.
    // Drawback: does not work if you reload the scene...
    // Update: get_node doesn't work if you reload the scene either, since it doesn't recognize the node was swapped out...
    // So no more signals come in... solution: call `:watch refresh`
    // NOTE: that will be fixed once we make await_untyped_signal detect the node being freed while waiting for it!
    // because then all watches will fail at the same time, causing them to re-evaluate, so it will re-connect the signals to the fresh node

    Rc::new(move || {
        let result = try {
            // We can do the callable in two ways: either:
            // 1. Return a future that resolves when the signal is emitted
            // 2. Return a signal immediately, which will then be awaited later by the evaluator
            // I think 2 is better, but I'm not sure, we can try both
            // 2 is more consistent with how it used to work before JobExpression::Callable existed

            let target_obj = if let Some(WatchablePath::NodePath(path)) = &path
                && !path.is_empty()  // <-- Warning! `path` can be empty if the Node is not in the tree!
                && !use_instance_id
            {
                // We can only use `get_node()` if the watchable object is a Node! Else, fallback to `instance_from_id()`.

                get_root()
                    .try_get_node_as::<Node>(path)
                    .ok_or_else(|| {
                        eyre!("failed to watch signal `{signal_name}` - node not found at `{path}`")
                    })?
                    .upcast::<Object>()
            } else {
                // Important: Gd::clone will panic if the object was freed, so we need to check its validity BEFORE cloning it.
                if !obj.is_instance_valid() {
                    Err(eyre!(
                        "failed to watch signal `{signal_name}` - target object was freed"
                    ))?; // Using ? on Err means: always bail out 
                }
                // Now it should be safe to clone it.
                obj.clone()
            };

            Variant::from(Signal::from_object_signal(&target_obj, &signal_name))
        };
        Box::pin(async move { result })
    })
}

/// Little helper struct only used for :watch signals
#[derive(Clone)] // cheap clone
pub(super) enum Watchable {
    Node(Gd<Node>),
    Resource(Gd<Resource>),
    Object(Gd<Object>),
}

// Do not #[derive(Debug)] here, or it will pollute the output with "NodePath" or "FilePath"
#[derive(Clone)]
pub(super) enum WatchablePath {
    NodePath(NodePath),
    FilePath(String),
}

// Basically equivalent to #[derive(Display)] if that existed
impl std::fmt::Display for WatchablePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchablePath::NodePath(p) => write!(f, "{p}"),
            WatchablePath::FilePath(p) => write!(f, "{p}"),
        }
    }
}

impl Watchable {
    pub(super) fn get_name(&self) -> String {
        match self {
            Watchable::Node(node) => node.get_name().to_string(),
            Watchable::Resource(res) => {
                let name = res.get_name();
                if name.is_empty() {
                    self.get_inner().get_class().to_string() // fallback to class name, if no name 
                } else {
                    name.to_string()
                }
            }
            Watchable::Object(obj) => obj.get_class().to_string(), // <-- Object has no Name, so just use its type for now
        }
    }

    pub(super) fn get_path(&self) -> Option<WatchablePath> {
        match self {
            Watchable::Node(node) => Some(WatchablePath::NodePath(node.get_path())), // <-- warning - this can be empty if the node is not in the tree
            Watchable::Resource(res) => Some(WatchablePath::FilePath(res.get_path().to_string())),
            Watchable::Object(_) => None,
        }
    }

    pub(super) fn get_inner(&self) -> Gd<Object> {
        match self {
            Watchable::Node(node) => node.clone().upcast(),
            Watchable::Resource(res) => res.clone().upcast(),
            Watchable::Object(obj) => obj.clone(),
        }
    }

    pub(super) fn get_kind(&self) -> &str {
        match self {
            Watchable::Node(_) => "node",
            Watchable::Resource(_) => "resource",
            Watchable::Object(_) => "object",
        }
    }
}
