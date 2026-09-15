use std::{cell::RefCell, fmt::Debug, hash::Hash, rc::Rc, sync::LazyLock};

use async_executor::LocalExecutor;
use bon::bon;
use godot::{
    self,
    obj::{Bounds, Gd, GdMut, GdRef, WithBaseField, bounds::DeclUser},
    prelude::GodotClass,
};
use indexmap::IndexMap;
use tap::Tap as _;
use tokio::runtime::{Builder, Runtime};

// Async Node v3

// Changelog:
// v3: add multiple task groups
// v2: add BoundTaskFactory?
// v1: initial version (02-06-2026)

pub type TaskGroups<T: AsyncNode> =
    Rc<RefCell<IndexMap<T::TaskGroupKey, Rc<LocalExecutor<'static>>>>>; // Unfortunately need double Rc here, to avoid double-borrow of self in tick_deferred

/// Allows you create bound tasks.
///
/// Note - don't keep the `BoundTaskFactory` around for too long, or it may cause problems if you use `self.stop_all_tasks()` later.
pub struct BoundTaskFactory<T: GodotClass + AsyncNode>(Gd<T>, TaskGroups<T>);

#[bon]
impl<T: GodotClass + AsyncNode> BoundTaskFactory<T> {
    /// Spawn a bound task on the executor. In this context, "bound" means "tied to the lifetime of the node", so the task stops when the node is freed.
    /// This is similar to the way Tweens in Godot can be bound to another node's lifetime.
    ///
    /// Note - this method does not overwrite the previous executor, so we can have multiple tasks running concurrently.
    #[tracing::instrument(skip_all)]
    #[allow(clippy::new_ret_no_self)]
    #[builder(start_fn = new, finish_fn = spawn)]
    pub fn setup<U: AsyncFnOnce(AsyncGd<T>) -> () + 'static>(
        &self,
        #[builder(start_fn)] future: U,
        #[builder(default)] group: T::TaskGroupKey,
    ) where
        T: GodotClass + WithBaseField,
    {
        // NOTE - we require the future to return () explicitly (`AsyncFnOnce(AsyncGd<T>) -> ()`).
        // Otherwise, it's very easy to accidentally mess it up so it doesn't actually await the future.
        // E.g. if you do bound_task().spawn(async |this| { async_fn(this) } ), the async_fn will NOT be awaited.
        // Because it's just an async closure that returns a Future that never gets polled.
        // So this will silently do the wrong thing at runtime = BAD!

        let this: AsyncGd<T> = AsyncGd(self.0.clone());
        let executor_dict = Rc::clone(&self.1); // Avoid double-borrow here by not calling get_or_init_executor

        executor_dict
            .borrow_mut()
            .entry(group)
            .or_default() // Default-construct the LocalExecutor
            .spawn(async { future(this).await })
            // .spawn(future(this)) // maybe cleaner? ensure it doesn't mess anything up though
            .detach();
    }
}

/// Implement this trait for your Godot class to make an async executor whose lifetime is bound to your class.
/// That means - the executor (and all its tasks) will automatically be stopped if the object gets freed.
/// This has the added advantage of making it safe to use `self` in the future, without having to check every time if `self` is destroyed.
/// Two other advantages of doing it this way:
/// 1. The tasks will be paused if you temporarily remove the node from the scene tree.
///    Very useful for FSMs or object pools!
/// 2. The tasks can be woken up by other threads, without requiring the `experimental-threads` feature.
///    Very useful for receiving messages on channels from other threads!
///
/// This behaves similarly to `Tween.bind_node()`, except we bind an async executor, instead of a tween.
/// Note - do not forget to call `self.tick_deferred()` in `process` (or `physics_process` if you want)
///
/// Also another thing to note: do not hold a `bind()/bind_mut()` across an await point:
/// this creates a long-lasting borrow, so it will likely panic in the next frame, since `_process(self)` calls `self.bind()`.
pub trait AsyncNode: WithBaseField {
    type TaskGroupKey: Eq + Hash + Default + Debug = (); // Use () to opt-out of task grouping, so you get a dictionary with only 1 entry

    fn bound_task(&mut self) -> BoundTaskFactory<Self> {
        // We call get_executors() here and store it separately, to avoid double-borrow inside of setup()
        // Note - this may cause subtle bugs if you keep the BoundTaskFactory around while someone calls stop_all_tasks()
        // Since then you will be spawning tasks on an executor that is no longer getting ticked
        BoundTaskFactory(self.to_gd(), Rc::clone(self.get_executors()))
    }

    // Don't need a setter anymore, since it has interior mutability now, we can mutate it via the getter.
    fn get_executors(&self) -> &TaskGroups<Self>; // TODO rename to get_task_groups?

    /// Cancels all the currently running async tasks instantly, across all groups.
    /// This is not recommended - ideally you want to pass a `CancellationToken` into every async task, so it can be cancelled gracefully.
    /// Otherwise the program may be left in a slightly invalid state.
    ///
    /// Update - if you're using tweens, you can use a `TweenGuard` to auto-stop the tween when the future is dropped.
    /// That makes the future cancel-safe.
    fn stop_all_tasks(&mut self) {
        self.get_executors().borrow_mut().clear();
    }

    /// Stop all tasks in a specific group.
    fn stop_tasks_in(&mut self, group: Self::TaskGroupKey) {
        // TODO in the future maybe make a variant that takes a predicate closure so you can do more sophisticated stuff.
        self.get_executors()
            .borrow_mut()
            .insert(group, Rc::default()); // Override the LocalExecutor with a new one, so it drops all futures that were running on the old one
    }

    /// Ticks the executor immediately. You should call this in either _`process()` or _`physics_process`.
    ///
    /// This method is pretty useless, since it only works if your future solely uses `bind()`, so no `bind_mut()` anywhere.
    /// Else, it will panic because both a mutable and an immutable borrow exists on `Self`.
    /// You probably want `tick_deferred()` instead, which works in a much broader context.
    fn tick_immediate(&self) {
        let dict = self.get_executors();

        for (_group, exec) in dict.borrow().iter() {
            let exec = Rc::clone(exec);
            tick_executor(&exec);
        }
    }

    /// Ticks the executor via `call_deferred`.
    ///
    /// You should call this in either _`process()` or _`physics_process`.
    /// Anything that happens in this tick should still happen on the same frame if you call this in `_process`.
    ///
    /// Note - this uses a while loop, so don't `yield_now` in your async task, else you get an infinite loop.
    /// `tick_executor` does have a per-frame tick-limit, so it should detect this, but still, don't do it.
    fn tick_deferred(&mut self) {
        let dict = Rc::clone(self.get_executors());

        for (_group, exec) in dict.borrow().iter() {
            let exec = Rc::clone(exec);

            // TODO - this will first tick the first executor x times, then the second one, then the third one, etc.
            // It would be better to interleave them so it becomes 12341234 instead of 11223344 methinks

            // This new method is visually cleaner, but requires tick_deferred() to take &mut self instead of &self.
            // You can see in the source code run_deferred_gd basically works exactly like the Callable thing we did before:
            // https://github.com/godot-rust/gdext/blob/e21dddfc3552d20a6c485b034935d674aac8fbc3/godot-core/src/obj/dyn_gd.rs#L429
            // Also notice run_deferred() just calls run_deferred_gd()
            self.run_deferred_gd(move |_this| tick_executor(&exec));
        }
    }
}

fn tick_executor(exec: &LocalExecutor<'static>) {
    let max_ticks = 100_000;
    let mut ticks = 0u32;
    while exec.try_tick() {
        // Do nothing.
        // See https://www.reddit.com/r/rust/comments/1k0f174/comment/mnfyr2l/

        ticks += 1;
        if ticks >= max_ticks {
            tracing::warn!(
                "Executor hit {max_ticks} tick limit, possible infinite loop, \
                             did you call yield_now()? If so, don't."
            );
            break;
        }
    }
}

/// Spawns a Rayon task that runs `F` and awaits it.
/// Returns `None` if `F` panicked (or future dropped mid-await? not sure)
pub fn spawn_rayon_with_result<R, F>(func: F) -> impl Future<Output = Option<R>>
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    let (tx, rx) = flume::unbounded();

    rayon::spawn(move || {
        let result = func();
        let sent = tx.send(result);

        if let Err(err) = sent {
            tracing::warn!(
                ?err,
                "Failed to send result of calculation in spawn_rayon_with_result"
            );
        }

        // tx dropped here
    });

    // Little hack to move rx into an async block, so we can return a reference to it
    async move { rx.recv_async().await.ok() }
}

//////////////////////////

/// `AsyncGd` is a wrapper around Gd that implements the nightly `Receiver` trait.
/// So you can safely use `Self` in an async method, e.g. `async fn foo(self: &AsyncGd<Self>) { ... }`
///
/// Note that we store `Gd<T>`, not `T`, since to get a `T` you need to call `bind()` on `Gd<T>`, which borrows it, so not suitable for long-term storage.
/// Otherwise you will get a borrow panic in the next frame.
///
/// Call `gd()` or `gd_mut()` to get access to the inner `Gd<T>`.
///
/// Note: `fn(self: AsyncGd<Gd<Self>>)` will NEVER work, as since it breaks the deref chain.
/// (`AsyncGd<Gd<T>>` derefs to `Gd<T>`, but `Gd<T>` does NOT deref to `T`, you have to explicitly call `bind()` to get a `T`.)
pub struct AsyncGd<T: GodotClass>(pub Gd<T>);

/// Manual impl of Clone, since the derive macro requires T: Clone, which we do not want.
impl<T: GodotClass> Clone for AsyncGd<T> {
    /// This is a cheap clone by the way.
    fn clone(&self) -> Self {
        Self(Gd::clone(&self.0))
    }
}

impl<T: GodotClass> AsyncGd<T> {
    /// Little helper so we can do `self.gd().queue_free()` instead of `self.0.queue_free()`, a little more clear what it means.
    pub const fn gd(&self) -> &Gd<T> {
        &self.0
    }

    /// Mutable equivalent of `gd()`.
    pub const fn gd_mut(&mut self) -> &mut Gd<T> {
        &mut self.0
    }
}

impl<T> AsyncGd<T>
where
    T: GodotClass + Bounds<Declarer = DeclUser>,
{
    /// Little helper so we can do `self.bind()` instead of `self.gd().bind()`.
    ///
    /// Note - this only works for user-declared classes, so not built-in ones like `Node3D`.
    /// That's why we need the `Bounds<Declarer = DeclUser>` bound.
    pub fn bind(&self) -> GdRef<'_, T> {
        self.gd().bind()
    }

    /// Mutable equivalent of `bind()`.
    pub fn bind_mut(&mut self) -> GdMut<'_, T> {
        self.gd_mut().bind_mut()
    }
}

/// This is the secret sauce that allows you to use `AsyncGd` as `self`.
impl<T: GodotClass> std::ops::Receiver for AsyncGd<T> {
    type Target = T;
}

// -------- //

/// Global Tokio executor we can use for background tasks that don't use the CPU much
pub static TOKIO_RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    tracing::info!("starting tokio runtime...");
    Builder::new_multi_thread()
        .name("tokyo")
        //    .worker_threads(4)
        .thread_name("tokiooo") // use a short name, or it will be cut off in Tracy/samply
        .enable_all() // Enable all tokio features
        .build()
        .unwrap()
        .tap(|_runtime| tracing::info!("started tokio runtime"))
});
