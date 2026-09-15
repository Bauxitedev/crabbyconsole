#![feature(duration_millis_float)]
#![feature(min_specialization)]
#![feature(arbitrary_self_types)]
#![feature(associated_type_defaults)]
#![feature(checked_type_aliases)]

use futures::future::Either;

#[cfg(feature = "tracy")]
pub mod tracy;

pub mod async_tween;
pub mod async_util;
pub mod flags;
pub mod gd;
pub mod logging;
pub mod profile;
pub mod rayon;
pub mod reflection;
pub mod stage;
pub mod util;

// Tracy stuff that can't be in the `tracy` module, otherwise they will be missing if that feature is disabled //

/// Check if tracy is enabled.
///
/// Note that just doing `#[cfg(feature = "tracy")]` is not enough, you actually do need `tracy_enabled()` as well.
/// Otherwise you may potentially run Tracy-related code in the editor.
#[tracing::instrument()]
pub fn tracy_enabled() -> bool {
    #[cfg(feature = "tracy")]
    {
        // Only use tracy if we're NOT in the editor.
        // if slow, maybe cache this in a thread_local LazyLock?

        use godot::{classes::Engine, obj::Singleton as _};

        !Engine::singleton().is_editor_hint()
    }
    #[cfg(not(feature = "tracy"))]
    {
        // If the tracy feature is disabled, always return false.
        false
    }
}

pub trait FutureTracyExt: Future + Sized {
    /// Wrap the future in a Tracy non-continuous-frame.
    ///
    /// This is useful for futures that run on the main thread, so we can't use `instrument()` for regular spans.
    ///
    /// The name is leaked but cached, so passing the same name twice should not leak it again.
    /// Additionally, a number is appended to the name, depending on how many of the futures are active simultaneously.
    ///
    /// After more search, I discovered futures that don't run on the main thread seem to struggle with `instrument()` as well.
    /// It only shows the busy time of the future, not the idle time, so a long-lasting future is cut into many small chunks. Not very useful!
    ///
    /// Note - The non-continuous frame feature is actually supposed to be used for short-lived frames (<16ms).
    /// So we're misusing that feature here, until `tracy_client` gets fiber support: <https://github.com/nagisa/rust_tracy_client/issues/35>
    /// Then we can switch to using fibers instead of non-continuous frames.
    ///
    /// If the `tracy` feature is disabled, this does nothing.
    fn with_tracy_non_continuous_frame(
        self,
        job_pool_name: &str,
    ) -> impl Future<Output = Self::Output>;

    /// Same as [`with_tracy_non_continuous_frame`], but only wraps the future if `condition` is true.
    ///
    /// Useful when the tracy frame should only be created conditionally at the call site.
    fn with_tracy_non_continuous_frame_if(
        self,
        condition: bool,
        job_pool_name: &str,
    ) -> impl Future<Output = Self::Output> {
        if condition {
            Either::Left(self.with_tracy_non_continuous_frame(job_pool_name))
        } else {
            Either::Right(self)
        }
    }
}

impl<F: Future> FutureTracyExt for F {
    fn with_tracy_non_continuous_frame(
        self,
        job_pool_name: &str,
    ) -> impl Future<Output = Self::Output> {
        #[cfg(feature = "tracy")]
        let (id, frame_name) = {
            use crate::tracy::acquire_task_id;

            let id = acquire_task_id(job_pool_name);

            (
                id,
                crate::tracy::cache_frame_name(&format!("{job_pool_name}[{id}]")),
            )
        };

        async move {
            #[cfg(feature = "tracy")]
            use crate::tracy::release_task_id;

            #[cfg(feature = "tracy")]
            let _guard =
                Some(crate::tracy::get_tracy_client_or_panic().non_continuous_frame(frame_name));

            #[cfg(feature = "tracy")]
            scopeguard::defer! {
                release_task_id(job_pool_name, id);
            }

            #[cfg(not(feature = "tracy"))]
            let _ = job_pool_name; // avoid "variable not used" warning

            self.await
        }
    }
}
