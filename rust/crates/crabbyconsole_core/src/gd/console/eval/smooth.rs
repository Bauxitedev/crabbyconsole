use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use color_eyre::eyre::{Ok, Report, eyre};
use crabbyconsole_clap::smooth::SmoothArgs;
use crabbyconsole_misc::{FutureTracyExt as _, gd::async_node::AsyncGd};
use godot::prelude::*;
use mini_moka::unsync::Cache;

use crate::gd::console::{
    CrabConsole, eval::ClapSubAction, job::JobExpressionInner, util::BoxedCache,
};

impl ClapSubAction for SmoothArgs {
    async fn handle(self, console: AsyncGd<CrabConsole>) -> Result<Variant, Report> {
        let Self { speed, expression } = self;

        thread_local! {

            /// TODO you should move this cache into CrabConsole!
            /// Otherwise, if this holds a Object-type, it will panic when you shut down the game (due to running Gd's destructor after Godot has already exited).
            static SMOOTH_CACHE: BoxedCache<Arc<str>, Rc<Variant>> = RefCell::new(Box::new(
                Cache::builder()
                    .time_to_live(Duration::from_mins(10))
                    .build()
            ));
        }

        // TODO maybe trim the start of the expression too? To increase cache hit rate
        let expression: Arc<str> = Arc::from(expression.join(" "));

        let new = console
            .clone() // fast clone
            .eval_job_without_channel(JobExpressionInner::String(Arc::clone(&expression)))
            .with_tracy_non_continuous_frame("eval_job_smooth")
            .await?;

        // Check if expression was present in cache
        let old = SMOOTH_CACHE.with(|c| c.borrow_mut().get(&expression).cloned());

        let Some(old) = old else {
            // Insert in cache
            // Variant cloned here, not sure how slow that is:                  vvvvv
            SMOOTH_CACHE.with(|c| c.borrow_mut().insert(expression, Rc::new(new.clone())));

            tracing::info!("inserted in smooth cache");

            // Return unsmoothed if we don't have a cached entry
            return Ok(new);
        };

        // TODO this does not work properly in slow motion
        let delta = console.gd().get_process_delta_time();

        // for extra smoothness you can do :smooth :smooth randf() -> looks almost like perlin noise
        let smoothed =
            variant_smooth(&old, &new, speed, delta).ok_or_else(|| error_wrong_type(&new))?;

        // Insert in cache
        // Variant cloned here, not sure how slow that is:                       vvvvv
        SMOOTH_CACHE.with(|c| c.borrow_mut().insert(expression, Rc::new(smoothed.clone())));

        Ok(smoothed)
    }
}

// this is just lerp smooth with fixed delta
fn variant_smooth(a: &Variant, b: &Variant, lerp_speed: f64, delta: f64) -> Option<Variant> {
    let t = (1.0 - (-delta * lerp_speed).exp()).clamp(0.0, 1.0);

    // manual lerp: a + (b - a) * t
    let result = a.evaluate(
        &b.evaluate(a, VariantOperator::SUBTRACT)?
            .evaluate(&Variant::from(t), VariantOperator::MULTIPLY)?,
        VariantOperator::ADD,
    )?;

    Some(result)
}

// TODO copy pasted from delta.rs
fn error_wrong_type(var: &Variant) -> Report {
    eyre!(
        "`:smooth` expected numeric type, instead got {:?}",
        var.get_type()
    )
}
