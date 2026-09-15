use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use color_eyre::eyre::{Report, eyre};
use crabbyconsole_clap::delta::DeltaArgs;
use crabbyconsole_misc::{FutureTracyExt as _, gd::async_node::AsyncGd};
use godot::{builtin::math::ApproxEq, prelude::*};
use mini_moka::unsync::Cache;

use crate::gd::console::{
    CrabConsole, eval::ClapSubAction, job::JobExpressionInner, util::BoxedCache,
};

impl ClapSubAction for DeltaArgs {
    async fn handle(self, console: AsyncGd<CrabConsole>) -> Result<Variant, Report> {
        let Self {
            expression,
            norm,
            dt,
        } = self;
        // This is a thread-local, non-thread-safe cache, since it contains Variant, which is !Send.
        thread_local! {

            /// Note this is a global cache, so it is NOT cleared when you reload the console.
            /// Warning! `Cache` is a big struct, like 300 bytes just for an empty cache.
            /// If you store it raw in a thread_local!, it will blow up because it doesn't fit in the TLS buffer.
            /// Solution: Box it!
            ///
            /// TODO you should move this cache into CrabConsole!
            /// Otherwise, if this holds a Object-type, it will panic when you shut down the game
            /// (due to running Gd's destructor after Godot has already exited).
            static DELTA_CACHE: BoxedCache<Arc<str>, Rc<(Variant, Instant)>> = RefCell::new(Box::new(
                Cache::builder() // Using Rc because I don't know how expensive it is to clone a Variant
                    .time_to_live(Duration::from_mins(10)) // We assume nobody makes a :watch with --rate 0.00166... or lower
                    .build()
            ));
        }

        let expression = expression.join(" "); // TODO maybe trim the start of the expression too? To increase cache hit rate
        let expression: Arc<str> = expression.into();

        let new = console
            .clone() // fast clone
            .eval_job_without_channel(JobExpressionInner::String(expression.clone()))
            .with_tracy_non_continuous_frame("eval_job_diff")
            .await?;
        let now = Instant::now();

        // Check if expression was present in cache
        let old = DELTA_CACHE.with(|c| c.borrow_mut().get(&expression).cloned());

        let Some(old) = old else {
            let identity = variant_identity(&new).ok_or_else(|| error_wrong_type(&new))?;
            tracing::info!(?identity);

            // Insert in cache
            DELTA_CACHE.with(|c| c.borrow_mut().insert(expression, Rc::new((new, now))));

            // We return the identity the expression's type, to avoid returning a too high value the first time you call :delta
            // This prevents a spike in the plot if you do :line plot :delta <expr>
            if norm {
                return Ok(Variant::from(
                    try_norm(&identity).ok_or_else(|| error_norm_wrong_type(&identity))?,
                ));
            } else {
                return Ok(identity);
            }

            // TODO - duplicate code here, but we can't merge the branches, since otherwise the first :delta call will return too high of a value
            // The first :delta call must return zero
        };

        // Insert in cache
        let new = Rc::from((new, now));
        DELTA_CACHE.with(|c| c.borrow_mut().insert(expression, Rc::clone(&new)));

        // If dt is set, multiply by 1.0 / dt
        // Multiplication should be faster than division for vectors (since we have only 1 division instead of 2 or more)

        let use_godot_delta = false;
        let time_delta = if use_godot_delta {
            console.bind().base().get_process_delta_time()
            // Godot uses delta smoothing, which means the result is far more smooth.
            // However, it WILL be influenced by slow motion, so maybe divide by time scale here?
            // TODO also this goes wrong if you rate-limit the watch, e.g --rate 6 makes the resulting value 10x too big!
            // Can we count how many frames ago the result was calculated? Or sum the delta times?
        } else {
            (new.1 - old.1).as_secs_f64() // Most accurate, uses wall-clock time, but very jittery and unstable
        };

        let multiplier = if dt { Some(1.0 / time_delta) } else { None };

        // Now try to delta new and old (new - old) and optionally multiply by multiplier
        let result =
            variant_delta(&new.0, &old.0, multiplier).ok_or_else(|| error_wrong_type(&new.0))?;

        // If user requested to calculate the norm, do it now (may fail if incompatible type)
        if norm {
            Ok(Variant::from(
                try_norm(&result).ok_or_else(|| error_norm_wrong_type(&result))?,
            ))
        } else {
            Ok(result)
        }
    }
}

fn error_wrong_type(var: &Variant) -> Report {
    eyre!(
        "`:delta` expected numeric type, instead got {:?}",
        var.get_type()
    )
}
fn error_norm_wrong_type(var: &Variant) -> Report {
    eyre!(
        "the `--norm` flag can only be used with numeric types, Vector-like types, and Quat, not {:?}",
        var.get_type()
    )
}

/// Given a Variant, compute its numerical identity.
/// E.g.:
/// - 5 -> 0
/// - 5.0 -> 0.0
/// - Vector3(1,2,3) -> Vector3(0,0,0)
/// - Quaternion(0,1,0,0) -> Quaternion(0,0,0,1)
///
/// Et cetera. Returns `None` if we have a non-numerical type.
fn variant_identity(var: &Variant) -> Option<Variant> {
    match var.get_type() {
        VariantType::QUATERNION => Some(Quaternion::IDENTITY.to_variant()),

        // For matrices, the logical equivalent of "delta" would be:
        // delta = a.affine_inverse() * b

        // Don't think we need these
        //   VariantType::BASIS => Some(Basis::IDENTITY.to_variant()),
        //   VariantType::TRANSFORM2D => Some(Transform2D::IDENTITY.to_variant()),
        //   VariantType::TRANSFORM3D => Some(Transform3D::IDENTITY.to_variant()),
        _ => var.evaluate(var, VariantOperator::SUBTRACT),
    }
}

// Tries to find the difference between variant A and variant B, then optionally multiplies the result by `multiplier`.
fn variant_delta(a: &Variant, b: &Variant, multiplier: Option<f64>) -> Option<Variant> {
    match (a.get_type(), b.get_type()) {
        (VariantType::QUATERNION, VariantType::QUATERNION) => Some(Variant::from({
            // normalizing here to prevent floating point drift
            let mut delta =
                b.to::<Quaternion>().normalized() * a.to::<Quaternion>().normalized().inverse();

            if let Some(multiplier) = multiplier {
                // to "scale" a quat by a float you can do
                // Quaternion.IDENTITY.slerp(quat, x)
                // yes, this works even if x < 0 or x > 1.
                delta = Quaternion::IDENTITY.slerp(delta, multiplier as f32);
            }
            delta
        })),

        _ => {
            let mut delta = a.evaluate(b, VariantOperator::SUBTRACT)?; // a - b
            if let Some(multiplier) = multiplier {
                // We multiply here instead of dividing because
                // 1. more variants support it
                // 2. it's faster for things that have more than 1 scalar (e.g. a vector)
                delta = delta.evaluate(&Variant::from(multiplier), VariantOperator::MULTIPLY)?;
            }

            Some(delta)
        }
    }
}

/// Try to calculate the norm (aka magnitude) of a Variant
pub(super) fn try_norm(v: &Variant) -> Option<f32> {
    // match_class! is not gonna work, since Vector* is not a Class

    // There's probably a less tedious way to write this - maybe a trait?
    if let Ok(v) = v.try_to::<f64>() {
        // VariantType::FLOAT always stores a f64
        Some(v.abs() as f32)
    } else if let Ok(v) = v.try_to::<i64>() {
        // VariantType::INT always stores a i64
        Some(v.abs() as f32)
    } else if let Ok(v) = v.try_to::<Vector2>() {
        Some(v.length())
    } else if let Ok(v) = v.try_to::<Vector2i>() {
        Some(v.length())
    } else if let Ok(v) = v.try_to::<Vector3>() {
        Some(v.length())
    } else if let Ok(v) = v.try_to::<Vector3i>() {
        Some(v.length())
    } else if let Ok(v) = v.try_to::<Vector4>() {
        Some(v.length())
    } else if let Ok(v) = v.try_to::<Vector4i>() {
        Some(v.length())
    } else if let Ok(v) = v.try_to::<Quaternion>() {
        let v = if v.length_squared().approx_eq(&0.0) {
            Quaternion::IDENTITY // important - prevent panic due to normalizing zero quat
        } else {
            v.normalized()
        };

        // Not necessarily mathematically correct - v.length() would be more correct
        // but in practice it's never useful, since we assume quats are always normalized in the gamedev world

        Some(stable_angle(v)) // more numerically stable than v.get_angle()
    } else {
        None
    }
}

/// More numerically stable version of `Quaternion::get_angle()`.
/// That one gets unstable and jumps from 0.001 -> 0.0007 -> 0.0005 -> 0
fn stable_angle(q: Quaternion) -> f32 {
    let len = (q.x * q.x + q.y * q.y + q.z * q.z).sqrt();
    2. * len.atan2(q.w)
}
