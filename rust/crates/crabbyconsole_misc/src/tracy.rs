use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};

use godot::{
    classes::{Performance, performance::Monitor},
    obj::Singleton as _,
};
use tracing_tracy::client::{self, Client, PlotConfiguration, PlotFormat, PlotName};

use crate::util::get_process_ram_bytes;

const PLOT_FRAME_TIME: PlotName = client::plot_name!("Frame Time (ms)");

const PLOT_MEMORY_STATIC: PlotName = client::plot_name!("Static Memory Usage"); // Godot 
const PLOT_MEMORY_RUST: PlotName = client::plot_name!("Rust Memory Usage"); // Rust
const PLOT_RENDER_VIDEO_MEM_USED: PlotName = client::plot_name!("VRAM Usage"); // Godot 

const PLOT_OBJECTS: PlotName = client::plot_name!("Objects"); // Godot 
const PLOT_RESOURCES: PlotName = client::plot_name!("Resources"); // Godot 
const PLOT_NODES: PlotName = client::plot_name!("Nodes"); // Godot 

const PLOT_PIPELINE_COMPILATIONS_CANVAS: PlotName = client::plot_name!("Compilations Canvas"); // Godot 
const PLOT_PIPELINE_COMPILATIONS_MESH: PlotName = client::plot_name!("Compilations Mesh"); // Godot 
const PLOT_PIPELINE_COMPILATIONS_SURFACE: PlotName = client::plot_name!("Compilations Surface"); // Godot 
const PLOT_PIPELINE_COMPILATIONS_DRAW: PlotName = client::plot_name!("Compilations Draw"); // Godot 

/// Does what it says on the tin.
///
/// Does NOT start the Tracy client if it's not running, instead it panics.
/// That should make it easier to pinpoint problems with lazy initialization.
#[must_use]
pub fn get_tracy_client_or_panic() -> client::Client {
    client::Client::running().expect("tracy client not running")
}
pub fn setup_tracy() {
    tracing::info!("starting tracy client");
    client::Client::start();

    //Important - if the `demangle` feature of tracy_client is set, you must call this macro.
    //Otherwise, you will get ` undefined symbol: ___tracy_demangle` when you run the game.
    client::register_demangler!();

    let client = get_tracy_client_or_panic();

    // We do actually need `plot_config`, otherwise the order of the graphs is wrong.

    client.plot_config(PLOT_FRAME_TIME, PlotConfiguration::default());

    ////

    client.plot_config(
        PLOT_MEMORY_STATIC,
        PlotConfiguration::default().format(PlotFormat::Memory), //   .fill(false),
    );

    client.plot_config(
        PLOT_MEMORY_RUST,
        PlotConfiguration::default().format(PlotFormat::Memory),
    );

    client.plot_config(
        PLOT_RENDER_VIDEO_MEM_USED,
        PlotConfiguration::default().format(PlotFormat::Memory),
    );
}

thread_local! {
    static LAST_FRAME: RefCell<Instant> = RefCell::new(Instant::now());
}
/// Call this at the very end of each frame
#[tracing::instrument(skip_all)]
pub fn update_tracy_and_mark_frame() {
    let client = get_tracy_client_or_panic();

    // Setup plots, see https://github.com/nagisa/rust_tracy_client/blob/main/examples/src/plots/mod.rs

    if Client::is_connected() {
        let perf = Performance::singleton();
        client.plot(PLOT_MEMORY_STATIC, perf.get_monitor(Monitor::MEMORY_STATIC));
        client.plot(PLOT_MEMORY_RUST, get_process_ram_bytes() as f64); // <-- This is very slow on Windows (5ms+ per frame)
        client.plot(
            PLOT_RENDER_VIDEO_MEM_USED,
            perf.get_monitor(Monitor::RENDER_VIDEO_MEM_USED),
        );
        ////////////

        client.plot(PLOT_OBJECTS, perf.get_monitor(Monitor::OBJECT_COUNT));
        client.plot(
            PLOT_RESOURCES,
            perf.get_monitor(Monitor::OBJECT_RESOURCE_COUNT),
        );
        client.plot(PLOT_NODES, perf.get_monitor(Monitor::OBJECT_NODE_COUNT));

        ////////////

        client.plot(
            PLOT_PIPELINE_COMPILATIONS_CANVAS,
            perf.get_monitor(Monitor::PIPELINE_COMPILATIONS_CANVAS),
        );
        client.plot(
            PLOT_PIPELINE_COMPILATIONS_MESH,
            perf.get_monitor(Monitor::PIPELINE_COMPILATIONS_MESH),
        );
        client.plot(
            PLOT_PIPELINE_COMPILATIONS_SURFACE,
            perf.get_monitor(Monitor::PIPELINE_COMPILATIONS_SURFACE),
        );
        client.plot(
            PLOT_PIPELINE_COMPILATIONS_DRAW,
            perf.get_monitor(Monitor::PIPELINE_COMPILATIONS_DRAW),
        );
    }

    // Measure time since last frame
    let now = Instant::now();
    let frame_time = LAST_FRAME.with(|last_frame| now - *(*last_frame).borrow());
    client.plot(PLOT_FRAME_TIME, frame_time.as_millis_f64());
    LAST_FRAME.replace(now);

    // Update - now we first plot and THEN frame mark. May work better for Rust memory usage.
    // Also, memory usage is slowly going up, but that could be caused by Tracy itself as well, since we're collecting a LOT of data.

    // This marks the boundary between two continuous frames. You should call it at the END of each frame.
    // Note - I'm not sure when in the frame on_main_loop_frame() runs, so this frame mark may be in-between frames
    // Update - according to the docs: "It runs after all process() methods on Node, and before the Godot-internal ScriptServer::frame()"
    // So yeah that's no good... Allegedly MainLoop::process runs very early, can we hook into that somehow?
    client::frame_mark();
    /*
    frame_mark:  Indicate that rendering of a continuous frame has __ended__.
    In a traditional rendering scenarios a frame mark should be inserted after a buffer swap.
    */
}

thread_local! {
    // Note - HashMap is big (48 bytes on stack), so always Box/Rc it in TLS to prevent crash
    static FRAME_NAMES: Rc<RefCell<HashMap<String, tracing_tracy::client::FrameName>>> =
        Rc::default();
    static PLOT_NAMES: Rc<RefCell<HashMap<String, tracing_tracy::client::PlotName>>> =
        Rc::default();
}

/// Leaks the name, but caches the leaked name, so it will only leak it once.
///
/// This is needed to pass names to Tracy, without causing a big memory leak.
pub fn cache_frame_name(n: &str) -> tracing_tracy::client::FrameName {
    FRAME_NAMES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(&name) = cache.get(n) {
            return name;
        }

        let name = tracing_tracy::client::FrameName::new_leak(n.to_owned());
        cache.insert(n.to_string(), name);
        name
    })
}

pub fn cache_plot_name(n: &str) -> tracing_tracy::client::PlotName {
    PLOT_NAMES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(&name) = cache.get(n) {
            return name;
        }

        let name = tracing_tracy::client::PlotName::new_leak(n.to_owned());
        cache.insert(n.to_string(), name);
        name
    })
}

thread_local! {
    // Note - HashMap is big (48 bytes on stack), so always Box/Rc it in TLS
    static TASK_NAME_POOLS: Rc<RefCell<HashMap<String, IdPool>>> =
        Rc::default();
}

// -------- //

struct IdPool {
    free: Vec<usize>,
    next: usize,
}

impl IdPool {
    const fn new() -> Self {
        Self {
            free: Vec::new(),
            next: 0,
        }
    }

    fn acquire(&mut self) -> usize {
        self.free.pop().unwrap_or_else(|| {
            let id = self.next;
            self.next += 1;
            id
        })
    }

    fn release(&mut self, id: usize) {
        self.free.push(id);
    }
}

pub fn acquire_task_id(task_name: &str) -> usize {
    TASK_NAME_POOLS.with(|pools| {
        pools
            .borrow_mut()
            .entry(task_name.to_string())
            .or_insert_with(IdPool::new)
            .acquire()
    })
}

pub fn release_task_id(task_name: &str, id: usize) {
    TASK_NAME_POOLS.with(|pools| {
        if let Some(p) = pools.borrow_mut().get_mut(task_name) {
            p.release(id);
        }
    });
}
