//! Async helpers that allow you do perform tweens using async Rust.
//! Note: all futures here are !Send, since they are intended to be run on the main thread only.
#![allow(clippy::future_not_send)]

use bon::{Builder, bon};
use godot::{
    classes::{
        Camera3D, Label3D, StandardMaterial3D, Tween,
        tween::{EaseType, TransitionType, TweenPauseMode, TweenProcessMode},
    },
    prelude::*,
};

use crate::{FutureTracyExt as _, gd::async_node::AsyncGd};

// Warning - all futures here are !Send, so only use them in AsyncGd, not Tokio.

/// Little helper to stop a Tween when a future gets cancelled.
/// Seems to work pretty well so far, we don't need the `CancellationToken` anymore it seems.
/// Using `SceneTree::get_processed_tweens()` to see if this is actually working - seems to work perfectly!
///
/// TODO - we can replace `TweenGuard` this by `defer!` now right?
struct TweenGuard(Gd<Tween>);

impl Drop for TweenGuard {
    fn drop(&mut self) {
        if self.0.is_running() {
            tracing::warn!("TweenGuard dropped on active tween, stopping it...");
        }
        self.0.kill(); // Seems safe - does not panic when you quit the game
    }
}

/// Configuration for a tween animation - controls both playback behavior
/// (process mode, pause mode) and interpolation (easing, transition curve).
#[derive(Builder, Debug, Clone)]
pub struct TweenConfig {
    #[builder(default = TransitionType::LINEAR)] // LINEAR is the default in Godot as well
    pub transition_type: TransitionType,

    #[builder(default = EaseType::IN_OUT)] // IN_OUT is the default in Godot as well
    pub ease_type: EaseType,

    #[builder(default = TweenPauseMode::BOUND)] // BOUND is the default in Godot as well
    pub pause_mode: TweenPauseMode,

    #[builder(default = TweenProcessMode::IDLE)] // IDLE is the default in Godot as well
    pub process_mode: TweenProcessMode,
    // don't add `parallel` here, if you need that, just create two tweens and join() them in async land
}

impl Default for TweenConfig {
    fn default() -> Self {
        Self::smooth()
    }
}

impl TweenConfig {
    /// Sets up the given Tween according to this `TweenConfig`.
    pub fn apply(&self, tween: &mut Gd<Tween>) {
        tween.set_trans(self.transition_type);
        tween.set_ease(self.ease_type);
        tween.set_pause_mode(self.pause_mode);
        tween.set_process_mode(self.process_mode);
    }

    pub fn linear() -> Self {
        Self::builder()
            .transition_type(TransitionType::LINEAR)
            .build()
    }

    pub fn smooth() -> Self {
        Self::builder() // Default ease is IN_OUT
            .transition_type(TransitionType::QUINT)
            .build()
    }

    pub fn elastic() -> Self {
        Self::builder()
            .transition_type(TransitionType::ELASTIC)
            .ease_type(EaseType::OUT)
            .build()
    }
}

#[must_use]
pub struct AsyncTween<C: GodotClass> {
    binder: AsyncGd<C>, // The binder is the node to which the tween will be bound. So if the binder is freed, the tween will stop.
}

/// Manual impl to avoid getting the C: Clone bound
impl<C: GodotClass> Clone for AsyncTween<C> {
    fn clone(&self) -> Self {
        Self {
            binder: self.binder.clone(), // cheap clone
        }
    }
}

#[bon]
impl<C: GodotClass + Inherits<Node>> AsyncTween<C> {
    /// Move to a new position.
    #[builder]
    pub async fn move_to(
        &self,
        #[builder(start_fn)] node: &Gd<Node3D>,
        #[builder(start_fn)] to: Vector3,
        #[builder(start_fn)] duration: f32,
        from: Option<Vector3>,
        #[builder(default)] config: TweenConfig,
    ) {
        self.interpolate_to(&node.clone().upcast(), "global_position", to, duration)
            .maybe_from(from)
            .config(config)
            .call()
            .await;
    }

    /// Allows moving/rotating/scaling to a new Transform.
    /// Rotation is interpolated using quaternions, to avoid gimbal lock.
    /// However, that means you can't rotate more than 180 degrees at a time.
    /// (maybe add `rotate_to` that uses `global_rotation` to fix that)
    ///
    /// Note - do not pass a Transform with scale 0 in either to or from, or it will fail.
    /// TODO - maybe we can add a check for that? Maybe check orthonormality/determinant of the matrix
    #[builder]
    pub async fn transform_to(
        &self,
        #[builder(start_fn)] node: &Gd<Node3D>,
        #[builder(start_fn)] to: Transform3D,
        #[builder(start_fn)] duration: f32,
        from: Option<Transform3D>,
        #[builder(default)] config: TweenConfig,
    ) {
        self.interpolate_to(&node.clone().upcast(), "global_transform", to, duration)
            .maybe_from(from)
            .config(config)
            .call()
            .await;
    }

    /// Rotate towards the given rotation in radians (global).
    ///
    /// This uses Euler angles, not quaternions, so this enables rotating more than 180 degrees, but does suffer from gimbal lock.
    #[builder]
    pub async fn rotate_to(
        &self,
        #[builder(start_fn)] node: &Gd<Node3D>,
        #[builder(start_fn)] to: Vector3,
        #[builder(start_fn)] duration: f32,
        from: Option<Vector3>,
        #[builder(default)] config: TweenConfig,
    ) {
        self.interpolate_to(&node.clone().upcast(), "global_rotation", to, duration)
            .maybe_from(from)
            .config(config)
            .call()
            .await;
    }

    pub async fn fade_to(
        &self,
        material: &Gd<StandardMaterial3D>,
        to: Color,
        duration: f32,
        config: TweenConfig,
    ) {
        // TODO add builder pattern
        // TODO support passing a start color as well

        self.interpolate_to(&material.clone().upcast(), "albedo_color", to, duration)
            // .maybe_from(from)
            .config(config)
            .call()
            .await;
    }

    #[builder]
    pub async fn fade_to_2d(
        &self,
        #[builder(start_fn)] node: &Gd<Label3D>,
        #[builder(start_fn)] to: Color,
        #[builder(start_fn)] duration: f32,
        from: Option<Color>,
        #[builder(default)] config: TweenConfig,
    ) {
        self.interpolate_to(&node.clone().upcast(), "modulate", to, duration)
            .maybe_from(from)
            .config(config)
            .call()
            .await;
    }

    #[builder]
    pub async fn fov_to(
        &self,
        #[builder(start_fn)] node: &Gd<Camera3D>,
        #[builder(start_fn)] to: f32,
        #[builder(start_fn)] duration: f32,
        from: Option<f32>,
        #[builder(default)] config: TweenConfig,
    ) {
        self.interpolate_to(&node.clone().upcast(), "fov", to, duration)
            .maybe_from(from)
            .config(config)
            .call()
            .await;
    }

    /// Makes a tween that calls a callback for every frame during the tween, where 0.0 = start and 1.0 = end.
    /// You can pass a `TweenConfig` to make the transition non-linear.
    #[builder]
    pub async fn method_to(
        &self,
        #[builder(start_fn)] mut callback: impl FnMut(f32) + 'static,
        #[builder(start_fn)] duration: f32,
        #[builder(default)] config: TweenConfig,
    ) {
        let callable = Callable::from_fn("tween_callback", move |x| callback(x[0].to()));

        self.generic_to("method", move |mut tween| {
            config.apply(&mut tween);

            tween.tween_method(
                &callable,
                &Variant::from(0.0),
                &Variant::from(1.0),
                duration.into(),
            );
        })
        .await;
    }

    /// Interpolates a generic property. We use a generic T here to enforce "to" and "from" have the same type.
    #[builder]
    pub async fn interpolate_to<T: ToGodot>(
        &self,
        #[builder(start_fn)] obj: &Gd<Object>,
        #[builder(start_fn)] property_name: &str,
        #[builder(start_fn)] to: T,
        #[builder(start_fn)] duration: f32,
        from: Option<T>,
        #[builder(default)] config: TweenConfig,
    ) {
        // Only Nodes have names. If the object is not a node, just return the thing to string (is longer and uglier, so not the default).
        let object_name = match obj.clone().try_cast::<Node>() {
            Ok(node) => node.get_name().to_string(),
            Err(obj) => obj.to_string(),
        };
        self.generic_to(&object_name, move |mut tween| {
            config.apply(&mut tween);

            let mut result =
                tween.tween_property(obj, property_name, &to.to_variant(), duration.into());
            if let Some(from) = from {
                result.from(&from.to_variant());
            }
        })
        .await;
    }

    /// Allows you to use Godot Tween's full range of possibilities, while waiting for it to finish.
    /// `FnOnce` is the "broadest" closure type: every closure implements `FnOnce`.
    ///
    /// `tween_name` is only used for Tracy for now.
    pub async fn generic_to(&self, tween_name: &str, setup_tween: impl FnOnce(Gd<Tween>)) {
        let tween = self.binder.gd().clone().upcast::<Node>().create_tween();
        setup_tween(tween.clone());
        let _guard = TweenGuard(tween.clone()); // Stops the tween if this future is cancelled
        let _ = tween
            .signals()
            .finished()
            .to_fallible_future()
            .with_tracy_non_continuous_frame(&format!("anim_tween_{tween_name}"))
            .await; // Ignore the error

        // Note - we want to_fallible_future(), not to_future(), since the latter panics when the signal object (the tween) is freed before the signal is fired
        // This can happen if e.g. the future is cancelled half-way through the animation, which drops the TweenGuard, which frees the tween.
        // It can also happen if `self` gets freed during the animation, since the tween is bound to `self`.
    }
}

/// This is a trait now, to make it more decoupled from `AsyncGd`.
pub trait AsyncAnim<T>
where
    T: GodotClass + Inherits<Node>,
{
    /// Create a tween builder bound to this node. The tween will stop automatically if the future is cancelled.
    ///
    /// Note - this only works if `T` in `AsyncGd<T>` is a `Node`, otherwise we can't call `create_tween` on it.
    ///
    /// To be fair, using `AsyncGd` on anything but a `Node` seems pretty useless, e.g. `Resources` don't have a `process()` callback, so they can't have an async executor anyway.
    fn async_anim(&self) -> AsyncTween<T>;
}
impl<T> AsyncAnim<T> for AsyncGd<T>
where
    T: GodotClass + Inherits<Node>,
{
    fn async_anim(&self) -> AsyncTween<T> {
        AsyncTween {
            binder: self.clone(),
        }
    }
}
