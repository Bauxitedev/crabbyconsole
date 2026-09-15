//! Async helpers that allow you do various things using async Rust.
//! Note: all futures here are !Send, since they are intended to be run on the main thread only.
#![allow(clippy::future_not_send)]

use std::{
    cell::LazyCell,
    convert::Infallible,
    ops::ControlFlow,
    pin::Pin,
    task::{Poll, Waker},
    time::Duration,
};

use futures::task::Context;
use godot::{
    classes::{Input, InputEvent, InputEventKey, SceneTreeTimer, object::ConnectFlags},
    global::Key,
    init::is_main_thread,
    prelude::*,
};
use tokio::time::error::Elapsed;

use crate::{
    gd::{async_node::TOKIO_RUNTIME, autoload::input_event_manager::InputEventManager},
    util::{get_root, get_scene_tree},
};

// Warning - all futures here are !Send, so only use them in AsyncGd, not Tokio.

// TODO - these combinators probably don't work consistently when pausing the game!
// E.g. wait_for_input_action_pressed()  will trigger when paused, but wait_for_key_typed/pressed may NOT.
// Because they depend on an autoload to send signals, which may not be triggered while paused.
// -> should be fixed now, I set process mode of InputEventManager to never pause!

/// Wait for a duration in Godot-time. (may not be real time, can be influenced by things like movie movie and lag)
/// Set `ignore_time_scale` to true to make things happen in real-time, ignoring slow motion.
///
/// This should be relatively fast, according to the Godot docs:
///
/// > "As opposed to regular `Timer`, `SceneTreeTimer` does not require the instantiation of a node."
pub fn wait_for_duration(
    time: f64,
    ignore_time_scale: bool,
) -> TimerFuture<impl Future<Output = ()>> {
    let timer = get_scene_tree()
        .create_timer_ex(time)
        .ignore_time_scale(ignore_time_scale)
        .done();

    let timeout = timer.signals().timeout().to_future();
    TimerFuture { timer, timeout }
}

/// This struct is needed so the user of `wait_for_duration()` gets access to the timer, so we can get the time left.
pub struct TimerFuture<F: Future<Output = ()>> {
    pub timer: Gd<SceneTreeTimer>,
    pub timeout: F,
    // In the future you could store the wait_time here as well, since SceneTreeTimer doesn't expose it (unlike the regular Timer)
}

/// Implementing `IntoFuture` on `TimerFuture` means we can `.await` it directly.
impl<F: Future<Output = ()>> IntoFuture for TimerFuture<F> {
    type Output = ();
    type IntoFuture = F;

    fn into_future(self) -> F {
        self.timeout
    }
}

pub async fn wait_for_next_frame() {
    get_scene_tree().signals().process_frame().to_future().await;
}

pub async fn wait_for_next_physics_frame() {
    get_scene_tree().signals().physics_frame().to_future().await;
}

/// Wait until a condition is true, checked once per frame.
/// If the condition is true already, does not await at all and continues execution immediately.
///
/// TODO - this is showing up in the profiler now...
/// Every time you call `wait_for_next_frame` it connects a signal, that part shows up in the profiler
pub async fn wait_until(mut condition: impl FnMut() -> bool) {
    while !condition() {
        wait_for_next_frame().await;
    }
}

thread_local! {
    /// TODO why use LazyCell here? thread_local is already lazy
    static GLOBAL_INPUT_EVENT_MANAGER: LazyCell<Gd<InputEventManager>> = LazyCell::new(|| {

        assert!(is_main_thread());

        tracing::info!("creating InputEventManager...");

        let mut root = get_root();
        let input_event_manager = InputEventManager::new_alloc();
        root.add_child(&input_event_manager); // Adds to end
        root.move_child(&input_event_manager, 0); // Move to start, to be before all other autoloads + the main scene

        // input_event_manager is automatically assigned a unique random name by Godot
        tracing::info!("created InputEventManager: {input_event_manager}");

        // Note: some Godot games may rely on the last subnode of /root/ being the current scene.
        // E.g. using current_scene = root.get_child(-1).
        // This is bad, fragile design, because you can just do get_tree().get_current_scene() instead.
        // However, since this official tutorials uses the bad method, people may use it in their games anyway.
        // See https://docs.godotengine.org/en/stable/tutorials/scripting/singletons_autoload.html#creating-the-script
        // Also read the comments below the tutorial for extra information.
        // So, to prevent this becoming an issue, we ensure our autoload is added `before` the current scene.

        // Also, it seems doing it this way makes it actually only emit the signal when the input is *actually* unhandled.
        // Which is exactly what we want.

        input_event_manager
    })
}

/// Waits for unhandled input events to come in and calls `effect` for every one.
/// This may be better than `wait_for_unhandled_input_event`, since it does not drop events.
pub async fn loop_over_unhandled_input_events<T>(
    mut effect: impl FnMut(Gd<InputEvent>) -> ControlFlow<T>,
) -> T {
    let iem = GLOBAL_INPUT_EVENT_MANAGER.with(|iem| Gd::clone(iem));

    let (tx, rx) = flume::unbounded();

    iem.signals().unhandled_input_event().connect(move |event| {
        let _ = tx.send(event);
    });

    while let Ok(event) = rx.recv_async().await {
        match effect(event) {
            ControlFlow::Continue(()) => { /* continue */ }
            ControlFlow::Break(value) => return value,
        }
    }

    // This can only be reached if not a single event was sent before the channel was dropped.
    // This seems impossible, because GLOBAL_INPUT_EVENT_MANAGER is a singleton.
    // So, that can only happen if you delete the singleton, which should never ever happen.
    unreachable!("InputEventManager was freed")
}

/// Waits for handled input events to come in and calls `effect` for every one.
pub async fn loop_over_handled_input_events<T>(
    mut effect: impl FnMut(Gd<InputEvent>) -> ControlFlow<T>,
) -> T {
    let iem = GLOBAL_INPUT_EVENT_MANAGER.with(|iem| Gd::clone(iem));

    let (tx, rx) = flume::unbounded();

    iem.signals().handled_input_event().connect(move |event| {
        let _ = tx.send(event);
    });

    while let Ok(event) = rx.recv_async().await {
        match effect(event) {
            ControlFlow::Continue(()) => { /* continue */ }
            ControlFlow::Break(value) => return value,
        }
    }

    // This can only be reached if not a single event was sent before the channel was dropped.
    // This seems impossible, because GLOBAL_INPUT_EVENT_MANAGER is a singleton.
    // So, that can only happen if you delete the singleton, which should never ever happen.
    unreachable!("InputEventManager was freed")
}

/// Waits for any (unhandled) key to be pressed. Returns the key that was pressed. (Does not trigger for echo events.)
pub async fn wait_for_any_key_pressed() -> Gd<InputEventKey> {
    loop_over_unhandled_input_events(|event| {
        if let Ok(key_event) = event.try_cast::<InputEventKey>()
            && key_event.is_pressed()
            && !key_event.is_echo()
        {
            return ControlFlow::Break(key_event);
        }
        ControlFlow::Continue(())
    })
    .await
}

/// Waits for given (unhandled) key to be pressed. (Does not trigger for echo events.)
pub async fn wait_for_key_pressed(keycode: Key) {
    loop_over_unhandled_input_events(|event| {
        if let Ok(key_event) = event.try_cast::<InputEventKey>()
            && key_event.is_pressed()
            && !key_event.is_echo()
            && key_event.get_keycode() == keycode
        {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    })
    .await;
}

/// Waits for given (unhandled) key to be released.
pub async fn wait_for_key_released(keycode: Key) {
    loop_over_unhandled_input_events(|event| {
        if let Ok(key_event) = event.try_cast::<InputEventKey>()
            && !key_event.is_pressed()
            && key_event.get_keycode() == keycode
        {
            // The !key_event.is_echo() check seems unneeded here, since I think only `pressed` events generate echoes
            return ControlFlow::Break(());
        }

        ControlFlow::Continue(())
    })
    .await;
}

/// Wait until a specific (unhandled) action is pressed.
/// Now checks if something (e.g. a textbox) consumed the input first.
pub async fn wait_for_input_action_pressed(action: &'static str) {
    loop_over_unhandled_input_events(|event| {
        if event.is_action_pressed(action) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .await;
}

/// Wait until a specific (unhandled) action is released.
/// Now checks if something (e.g. a textbox) consumed the input first.
pub async fn wait_for_input_action_released(action: &'static str) {
    loop_over_unhandled_input_events(|event| {
        if event.is_action_released(action) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .await;
}

/// Wait until a specific action is pressed.
/// Note: this does NOT check if something (e.g. a textbox) consumed the input first.
/// Integrate into `InputEventManager` if you want that.
pub async fn wait_for_input_action_pressed_handled(action: &str) {
    let input = Input::singleton();
    wait_until(|| input.is_action_just_pressed(action)).await;
}

/// Wait until a specific action is released.
/// Note: this does NOT check if something (e.g. a textbox) consumed the input first.
/// Integrate into `InputEventManager` if you want that.
pub async fn wait_for_input_action_released_handled(action: &str) {
    let input = Input::singleton();
    wait_until(|| input.is_action_just_released(action)).await;
}

/// Wait until a specific number of frames have passed
pub async fn wait_for_frames(count: u32) {
    for _ in 0..count {
        wait_for_next_frame().await;
    }
}

/// Waits until an input axis reaches within `threshold` of `target` (-1.0 to 1.0).
/// For digital inputs (keyboard/dpad), use a threshold of 0.0.
/// Note: this does NOT check if something (e.g. a textbox) consumed the input first.
/// Integrate into `InputEventManager` if you want that.
pub async fn wait_for_axis_threshold(
    negative_action: &str,
    positive_action: &str,
    target: f32,
    threshold: f32,
) {
    let input = Input::singleton();
    wait_until(|| {
        let axis = input.get_axis(&*negative_action, &*positive_action);
        (axis - target).abs() <= threshold
    })
    .await;
}

/// Waits until a specific (unhandled) letter key is pressed (case-insensitive) and then (optionally) released.
/// If the key was already pressed prior to calling this method, it should wait for it to be pressed again.
pub async fn wait_for_key_typed(key: char, wait_for_release: bool) {
    let keycode = Key::try_from_ord(key.to_ascii_uppercase() as i32).unwrap_or(Key::NONE);
    // TODO .unwrap_or(Key::NONE) causes NO key to be accepted, so it waits forever... may need to deal with that later

    wait_for_key_pressed(keycode).await;
    if wait_for_release {
        wait_for_key_released(keycode).await;
    }
}

/// Waits until each letter of a word is typed (unhandled)  in sequence (case-insensitive).
/// Does not wait for each key to be released.
pub async fn wait_for_word_typed(word: &str) {
    for ch in word.chars() {
        wait_for_key_typed(ch, false).await;
    }
}

//////////

/// This allows you optionally define a timeout for a future.
///
/// We use a trait here so the return type becomes `Result<T, Infallible>` if no timeout is used.
///
/// Update - actually that isn't gonna work because we can't store a `Job<NoTimeout>` in the same receiver as a `Job<Duration>`
pub trait TimeoutKind {
    type Error;
    fn run_with_timeout<F: Future>(
        &self,
        fut: F,
    ) -> impl Future<Output = Result<F::Output, Self::Error>>;
}

impl TimeoutKind for Duration {
    type Error = Elapsed;
    async fn run_with_timeout<F: Future>(&self, fut: F) -> Result<F::Output, Elapsed> {
        run_with_timeout(self, fut).await
    }
}

pub struct NoTimeout;

impl TimeoutKind for NoTimeout {
    type Error = Infallible;
    async fn run_with_timeout<F: Future>(&self, fut: F) -> Result<F::Output, Infallible> {
        Ok(fut.await)
    }
}

pub async fn maybe_timeout<S: TimeoutKind, F: Future>(
    kind: S,
    fut: F,
) -> Result<F::Output, S::Error> {
    kind.run_with_timeout(fut).await
}

pub enum MaybeTimeout {
    Timeout(Duration),
    NoTimeout,
}

impl TimeoutKind for MaybeTimeout {
    type Error = Elapsed;

    async fn run_with_timeout<F: Future>(&self, fut: F) -> Result<F::Output, Elapsed> {
        match self {
            Self::Timeout(duration) => run_with_timeout(duration, fut).await,
            Self::NoTimeout => Ok(fut.await),
        }
    }
}

async fn run_with_timeout<F: Future>(duration: &Duration, fut: F) -> Result<F::Output, Elapsed> {
    // let _guard = TOKIO_RUNTIME.enter(); // <-- don't do this - causes panic if called concurrently
    // instead: create the guard and drop it BEFORE awaiting
    // (we can't use tokio::spawn here, since `fut` is !Send)
    {
        let _guard = TOKIO_RUNTIME.enter();
        tokio::time::timeout(*duration, fut)
    }
    .await
}

impl From<f64> for MaybeTimeout {
    fn from(value: f64) -> Self {
        Self::Timeout(Duration::from_secs_f64(value))
    }
}

impl MaybeTimeout {
    pub fn unwrap_duration(&self) -> Duration {
        let Self::Timeout(duration) = self else {
            panic!("called `unwrap_duration` on `NoTimeout`")
        };
        *duration
    }
}

////////////

/// Classify as future as either sync or async by polling it once,
/// without needing a real executor. Does not drop or abandon a suspended future:
/// it's returned via `Classified::WasAsync`, so the caller can continue polling it.
pub fn classify_future<F: Future>(fut: F) -> FutureKind<F> {
    let mut fut = Box::pin(fut);
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => FutureKind::Sync(v),
        Poll::Pending => FutureKind::Async(fut),
    }
}

pub enum FutureKind<F: Future> {
    /// The future ran to completion on the very first poll: effectively synchronous.
    Sync(F::Output),
    /// The future returned `Poll::Pending` on the very first poll: classify it as asynchronous.
    Async(Pin<Box<F>>),
}

///////////

/// Asynchronously waits for a signal to be emitted.
///
/// TODO - if the node is freed while waiting for the signal, this will stall forever.
/// We will need some kind of mechanism to detect the node being freed, which is difficult.
/// Go take a peek at godot-rust's source code, in particular `FallibleFuture`, to see how they detect it.
///
/// Update: updating to godot 0.5.3 -> 0.5.5 did NOT fix it.
pub async fn await_untyped_signal(signal: Signal) -> Vec<Variant> {
    // signal.to_fallible_future().await; // <-- will not work - we don't know the amount of parameters beforehand

    let (tx, rx) = flume::unbounded();
    let callable = Callable::from_fn("await_untyped_signal", move |variants| {
        let variants_cloned = variants
            .iter()
            .map(|v| (*v).clone()) // Variant::clone should be fast (mostly refcount bumps)
            .collect::<Vec<_>>(); // Convert every &Variant to Variant by cloning it, otherwise we can't send it outside of this closure.
        let _ = tx.send(variants_cloned);
    });
    signal.connect_flags(
        &callable,
        ConnectFlags::ONE_SHOT, // Only fire once
    );

    rx.recv_async()
        .await
        .expect("await_untyped_signal panicked") // NOTE - if the node is freed you may be able to hit this panic as well
}
