use std::cell::OnceCell;

use godot::prelude::*;
#[cfg(feature = "tracy")]
use tracing_tracy::client::Span;

#[cfg(feature = "tracy")]
use crate::tracy_enabled;
use crate::util::get_root;

thread_local! {
    // This is safe, since Node is not refcounted, so the dtor should run before Godot exits
    static GLOBAL_TRACY_MANAGER: OnceCell<Gd<TracyManager>> = const { OnceCell::new() };
}

/// Helps send Tracy data. Does nothing if tracy is disabled.
///
/// TODO what happens if this is not the first autoload in the list? Does it still work correctly?
#[derive(GodotClass)]
#[class(base = Node)]
pub struct TracyManager {
    base: Base<Node>,

    #[cfg(feature = "tracy")]
    render_span: Option<Span>,
    #[cfg(feature = "tracy")]
    process_span: Option<Span>,
    #[cfg(feature = "tracy")]
    physics_process_span: Option<Span>,
    #[cfg(feature = "tracy")]
    process_deferred_span: Option<Span>,
    #[cfg(feature = "tracy")]
    a: OnReady<Gd<TracyCallbackNode>>,
    #[cfg(feature = "tracy")]
    b: OnReady<Gd<TracyCallbackNode>>,
}

#[godot_api]
impl INode for TracyManager {
    fn init(base: Base<Node>) -> Self {
        Self {
            base,
            #[cfg(feature = "tracy")]
            render_span: None,
            #[cfg(feature = "tracy")]
            process_span: None,
            #[cfg(feature = "tracy")]
            physics_process_span: None,
            #[cfg(feature = "tracy")]
            process_deferred_span: None,
            #[cfg(feature = "tracy")]
            a: OnReady::manual(),
            #[cfg(feature = "tracy")]
            b: OnReady::manual(),
        }
    }

    #[tracing::instrument(skip_all)]
    fn ready(&mut self) {
        // frame_post_draw is the very end of a frame (just before swap_buffers)
        // See https://github.com/godotengine/godot-docs/issues/9305

        #[cfg(feature = "tracy")]
        if tracy_enabled() {
            use godot::classes::RenderingServer;
            use tracing_tracy::client::span;

            tracing::info!("TracyManager readying up...");
            let result = GLOBAL_TRACY_MANAGER.with(|gtm| gtm.set(self.to_gd()));
            if result.is_err() {
                tracing::warn!("GLOBAL_TRACY_MANAGER was already setup, replacing it...");
            }

            let mut this = self.to_gd();

            let rs = RenderingServer::singleton();
            {
                let mut this = this.clone();
                rs.signals().frame_pre_draw().connect(move || {
                    let span = span!("render_cpu"); // tracy span
                    let old = this.bind_mut().render_span.replace(span);

                    if old.is_some() {
                        tracing::warn!(
                            "render_cpu span replaced while it was still active -
                    frame_pre_draw/frame_post_draw may not be called correctly"
                        );
                    }
                });
            }
            {
                let mut this = this.clone();
                rs.signals().frame_post_draw().connect(move || {
                    match this.bind_mut().render_span.take() {
                        Some(span) => drop(span), // stop the zone
                        None => tracing::warn!("render_cpu span was missing at frame_post_draw"),
                    }

                    crate::tracy::update_tracy_and_mark_frame(); //  do this at the very end of each frame
                });
            }

            // Now setup a/b nodes:
            // a runs before all other nodes
            // b runs after all other nodes

            {
                let mut this = this.clone();
                let mut this2 = this.clone();
                let mut this3 = this.clone();
                let mut a = TracyCallbackNode::new(
                    Some(Box::new(move |_| {
                        let span = span!("process_all_nodes");
                        let old = this.bind_mut().process_span.replace(span);

                        if old.is_some() {
                            tracing::warn!("process_span replaced while it was still active");
                        }
                    })),
                    Some(Box::new(move |_| {
                        let span = span!("physics_process_all_nodes");
                        let old = this2.bind_mut().physics_process_span.replace(span);

                        if old.is_some() {
                            tracing::warn!(
                                "physics_process_span replaced while it was still active"
                            );
                        }
                    })),
                    Some(Box::new(move |_| {
                        let span = span!("process_deferred_all_nodes");
                        let old = this3.bind_mut().process_deferred_span.replace(span);

                        if old.is_some() {
                            tracing::warn!(
                                "process_deferred_span replaced while it was still active"
                            );
                        }
                    })),
                );
                a.set_process_priority(i32::MIN);
                a.set_physics_process_priority(i32::MIN);
                self.base_mut().add_child(&a);
                self.a.init(a);
            }

            {
                let mut this2 = this.clone();
                let mut this3 = this.clone();

                let mut b = TracyCallbackNode::new(
                    Some(Box::new(move |_| {
                        match this.bind_mut().process_span.take() {
                            Some(span) => drop(span), // stop the zone
                            None => {
                                tracing::warn!("process_span span was missing at B::process");
                            }
                        }
                    })),
                    Some(Box::new(move |_| {
                        match this2.bind_mut().physics_process_span.take() {
                            Some(span) => drop(span), // stop the zone
                            None => {
                                tracing::warn!(
                                    "physics_process_span span was missing at B::physics_process"
                                );
                            }
                        }

                        // Also mark the physics process frame
                        // See https://github.com/godotengine/godot-docs/issues/9305
                        // (Remember we can have multiple physics frames per frame, if the FPS doesn't match up, or we have lag.)
                        // Better idea: make 2 nodes, A and B. one with min prio and one with max prio.
                        // call zone.start() in A.physics_process and zone.end() in B.physics_process.
                        // note this will ONLY measure your node's physics process, it will NOT measure the physics server step time
                        // (aka how much time is actually spend resolving physics bodies and stuff).
                        // it does seem that Godot exposes the physics server step time in the profiler though... maybe hook into that?

                        tracing_tracy::client::secondary_frame_mark!("physics_process");
                    })),
                    Some(Box::new(move |_| {
                        match this3.bind_mut().process_deferred_span.take() {
                            Some(span) => drop(span), // stop the zone
                            None => {
                                tracing::warn!(
                                    "process_deferred_span span was missing at B::process_deferred"
                                );
                            }
                        }
                    })),
                );
                b.set_process_priority(i32::MAX);
                b.set_physics_process_priority(i32::MAX);
                self.base_mut().add_child(&b);
                self.b.init(b);
            }
        }
    }
}

#[godot_api]
impl TracyManager {
    /// Plot a numeric value using Tracy.
    ///
    /// This is a static method now, so can be called even when there is no instance of `TracyManager` active.
    /// In that case, it will do nothing and print a warning.
    #[func]
    pub fn plot(name: String, value: f64) {
        #[cfg(feature = "tracy")]
        {
            use crate::tracy::{cache_plot_name, get_tracy_client_or_panic};

            if GLOBAL_TRACY_MANAGER.with(|gtm| gtm.get().is_none()) {
                tracing::warn!(
                    "GLOBAL_TRACY_MANAGER is not setup, please ensure the TracyManager autoload is in your game, else the trace will be missing frame markers"
                );
                return; // <-- we technically don't need this return here, but eh
            }

            let client = get_tracy_client_or_panic();
            client.plot(cache_plot_name(&name), value);
        }

        // Use the variables when `tracy` is disabled, to avoid unused variable warning.
        #[cfg(not(feature = "tracy"))]
        let _ = (name, value);
    }

    /// Creates an instance of `TracyManager` and adds it to the `/root/` node.
    ///
    /// Note - this does not work if you're calling it from `_ready` inside of another autoload.
    /// This is because `/root/` is still busy setting itself up then, so you can't mutate it.
    /// Either await one frame or use `call_deferred` for that.
    ///
    /// Note - this does mean it won't collect any plot data in the first frame,
    /// since `TracyManager::plot` will fail during the first frame.
    #[tracing::instrument(skip_all)]
    pub fn add_autoload_if_missing() {
        if GLOBAL_TRACY_MANAGER.with(|gtm| gtm.get().is_some()) {
            tracing::info!("GLOBAL_TRACY_MANAGER was already setup, no need to add it again");
            return;
        }

        tracing::info!("attaching TracyManager autoload to root...");

        // Note: add_child() will fail if we don't check for this.
        assert!(
            get_root().is_node_ready(),
            "root node wasn't ready yet, can't add the autoload right now - please wait one frame and try again"
        );
        let instance = Self::new_alloc();
        get_root().add_child(&instance);

        // TODO maybe slide it around so it becomes first in the autoload list?
        // May make the process time spans more accurate?
        // Or is that not needed because the subnodes use process priority to ensure they're always before/after all other nodes?

        // Note: Self::ready assigns itself to GLOBAL_TRACY_MANAGER, we don't do that in this method.
        // That way, it also works in case you manually add TracyManager to your game's autoload list.
        // So, it should also work without the console, and without calling add_autoload_if_missing to add the autoload.
    }
}

#[cfg(feature = "tracy")]
#[derive(GodotClass)]
#[class(base = Node, no_init)]
struct TracyCallbackNode {
    base: Base<Node>,

    callback_process: Option<Box<dyn FnMut(f64)>>,
    callback_physics_process: Option<Box<dyn FnMut(f64)>>,

    callback_process_deferred: Option<Box<dyn FnMut(f64)>>,
    //callback_physics_process_deferred too?
}

#[cfg(feature = "tracy")]
#[godot_api]
impl TracyCallbackNode {
    fn new(
        callback_process: Option<Box<dyn FnMut(f64)>>,
        callback_physics_process: Option<Box<dyn FnMut(f64)>>,
        callback_process_deferred: Option<Box<dyn FnMut(f64)>>,
    ) -> Gd<Self> {
        Gd::from_init_fn(|base| Self {
            base,
            callback_process,
            callback_physics_process,
            callback_process_deferred,
        })
    }
}

#[cfg(feature = "tracy")]
#[godot_api]
impl INode for TracyCallbackNode {
    fn ready(&mut self) {}
    fn process(&mut self, delta: f64) {
        if let Some(callback_process) = self.callback_process.as_mut() {
            (callback_process)(delta);
        }

        self.run_deferred(move |this| {
            if let Some(callback_process_deferred) = this.callback_process_deferred.as_mut() {
                (callback_process_deferred)(delta);
            }
        });
    }
    fn physics_process(&mut self, delta: f64) {
        if let Some(callback_physics_process) = self.callback_physics_process.as_mut() {
            (callback_physics_process)(delta);
        }
    }
}
