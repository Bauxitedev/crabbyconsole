//! Misc stuff the console needs to execute.
//! This looks like a candidate to move to a separate crate.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, BufWriter},
    path::PathBuf,
    sync::{Arc, LazyLock, OnceLock},
    time::Instant,
};

use ::image::{ImageBuffer, ImageFormat, Rgb};
use color_eyre::{
    Result as EyreResult,
    eyre::{self, Context as _, ContextCompat as _, Report, ensure, eyre},
};
use crabbyconsole_misc::{
    async_util::wait_for_next_frame,
    gd::async_node::spawn_rayon_with_result,
    profile,
    util::{generate_datetime_string, get_current_scene, get_root, get_viewport},
};
use godot::{
    classes::{
        image::Format as GodotImageFormat, node::ProcessMode, resource_loader::ThreadLoadStatus,
        text_server::AutowrapMode, *,
    },
    init::{GdextBuild, is_main_thread},
    prelude::*,
};
use http::StatusCode;
use mini_moka::unsync::Cache;
use scopeguard::defer;
use tokio::io::AsyncWriteExt as _;

/// Splits up a string at every whitespace character.
///
/// This works differently than e.g. bash, where you can do this: `foo a "b c"`.
/// So `a` is the first arg, and `b c` is the second one.
/// We do it differently: we instead treat `a` as the first arg, `"b` as the second, and `c"` as the third.
/// This is fully intentional. If you want arguments with spaces, you should use varargs.
/// Then, you can either parse the result manually, or feed it into the advanced GDscript evaluator.
pub(super) fn split_console_whitespace(line: &str) -> impl Iterator<Item = &str> {
    line.split_whitespace()
}

/// Reloads a node from disk.
pub(super) fn reload_node(mut node: Gd<Node>) -> EyreResult<()> {
    let mut parent = node.get_parent().context("node has no parent")?;
    let index = node.get_index();
    let scene_file = node.get_scene_file_path();

    ensure!(
        !scene_file.is_empty(),
        "tried to reload a node that wasn't loaded from disk"
    );

    node.queue_free();

    // Need to spawn a background task, since our async executor may stop after calling node.queue_free(), since we may have freed `self`.
    godot::task::spawn(async move {
        // Now wait 1 frame to ensure the previous one is gone.
        // Otherwise, they will co-exist, which causes many problems, and messes up the name of the second one.
        wait_for_next_frame().await;

        let res: EyreResult<()> = async move {
            let packed_scene = try_load::<PackedScene>(&scene_file.to_string())
                .with_context(|| format!("failed to load scene file `{scene_file}`"))?;
            let new_instance = packed_scene
                .instantiate()
                .context("failed to instantiate scene")?;

            parent.add_child(&new_instance.clone());
            parent.move_child(&new_instance, index);

            Ok(())
        }
        .await;
        match res {
            Ok(()) => {}
            Err(err) => tracing::warn!(?err, "failed to reload node"),
        }
    });

    Ok(())
}

/// Returns the world-space position along the cursor's ray at the given depth.
///
/// `depth` is the distance from the camera along the ray direction in world space.
pub(super) fn get_cursor_world_position(depth: f32, camera: Gd<Camera3D>) -> Vector3 {
    let viewport = camera
        .get_viewport()
        .expect("camera 3d has no viewport, ensure it's in the scene tree"); // TODO handle more gracefully?

    let mouse_pos = viewport.get_mouse_position();
    let origin = camera.project_ray_origin(mouse_pos);
    let direction = camera.project_ray_normal(mouse_pos);

    origin + direction * depth
}

/// Writes `size_bytes` bytes of zeroes to `out.bin`, overwriting it if it exists.
/// Logs timestamps around buffer allocation and around the actual write so you
/// can tell whether a lag spike comes from allocation or from IO.
pub(super) async fn write_zeroes(size_bytes: usize) -> std::io::Result<PathBuf> {
    // No need to enter a runtime here - write_zeroes is only spawned on the tokio runtime anyway

    let path = std::env::current_dir()?.join("out.bin");

    tracing::info!(size_bytes, path = %path.display(), "before allocating buffer");
    let alloc_start = Instant::now();

    // Allocate zero vec
    let buf = vec![0u8; size_bytes];

    tracing::info!(
        size_bytes = buf.len(),
        elapsed_ms = alloc_start.elapsed().as_millis(),
        "after allocating buffer"
    );

    tracing::info!(path = %path.display(), "before opening file");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true) // overwrite if it exists
        .open(&path)
        .await?;

    tracing::info!(
        size_bytes = buf.len(),
        path = %path.display(),
        "before writing to disk"
    );
    let io_start = Instant::now();

    file.write_all(&buf).await?;
    file.flush().await?;

    tracing::info!(
        size_bytes = buf.len(),
        path = %path.display(),
        elapsed_ms = io_start.elapsed().as_millis(),
        "after writing to disk"
    );

    Ok(path)
}

/// Performs a for loop from `start` to `end` (inclusive), stepping by `step`.
/// Will detect and prevent infinite loops and overflow.
pub(super) fn stepped_range(
    start: i64,
    end: i64,
    step: i64,
) -> eyre::Result<impl Iterator<Item = i64>> {
    ensure!(step != 0, "step must not be 0");
    ensure!(
        !((end > start && step < 0) || (end < start && step > 0)),
        "step {step} does not move from {start} toward {end}"
    );

    Ok(std::iter::successors(Some(start), move |&x| {
        let next = x.checked_add(step)?; // breaks if overflows
        if (step > 0 && next <= end) || (step < 0 && next >= end) {
            Some(next)
        } else {
            None
        }
    }))
}

pub(super) fn render_template(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{key}}}"), value);
    }
    result
}

pub(super) async fn http_get(url: &str) -> eyre::Result<String> {
    use godot::{classes::http_request::Result as HttpResult, global::Error as GodotError};

    // Create the HttpRequest node and attach it to the tree.
    let mut http = HttpRequest::new_alloc();
    http.set_process_mode(ProcessMode::ALWAYS); // Prevent a paused game from pausing the http request.

    // http.set_use_threads(true) seems to cause some kind of deadlock so don't use threads
    // to diagnose run :debug http-get https://eu.httpbin.org/
    // (or any other slow server that runs into the console timeout before it finishes)

    get_current_scene()
        .expect("no current scene")
        .add_child(&http);

    // Ensure we always clean up the node, even on early return.
    let cleanup_target = http.clone();
    scopeguard::defer! {
        if cleanup_target.is_instance_valid() {
            cleanup_target.clone().queue_free();
        }
    }

    // Grab the future for the `request_completed` signal *before* firing the request,
    // so we can't possibly miss the emission.
    let fut = http.signals().request_completed().to_fallible_future();

    let err = http.request(url);
    ensure!(
        err == GodotError::OK,
        "failed to start HTTP request: {err:?}"
    );

    // This can fail if we switch scenes during the HTTP request,
    // causing the HTTPRequest node to be freed.
    let (result, response_code, _headers, body) = fut.await?;

    let result = HttpResult::from_ord(result as i32);
    if result != HttpResult::SUCCESS {
        return Err(eyre!("HTTP request did not succeed: {:?}", result));
    }

    //200-299 = success
    //300-399 = redirection
    //400-499 = client error
    //500-599 = server error

    // Do not use is_client_error/is_server_error, we can only use it if response_code is a valid response code.
    // In the future the HTTP standard may get new kinds of client/server errors which may not be caught otherwise.
    if (400..600).contains(&response_code) {
        return Err(eyre!(
            "HTTP request returned status {}",
            describe_status(response_code)
        ));
    }

    String::from_utf8(body.to_vec()).map_err(|_| eyre!("response body was not valid UTF-8"))
}

/// Formats a numeric HTTP status code as e.g. "502 Bad Gateway", falling back to
/// just the number if it's not a code the `http` crate recognizes.
pub(super) fn describe_status(code: i64) -> String {
    match u16::try_from(code)
        .ok()
        .and_then(|c| StatusCode::from_u16(c).ok())
    {
        Some(status) => match status.canonical_reason() {
            Some(reason) => format!("{} {}", status.as_u16(), reason),
            None => status.as_u16().to_string(),
        },
        None => code.to_string(),
    }
}

///////////

/// Checks if a name is a valid `GDScript` identifier (also checks if it isn't a reserved keyword).
/// NOTE - this is similar to `StringName::is_valid_identifier`, except that one doesn't check for reserved keywords.
/// It's better to make our own method for this, to ensure our logic is consistent across different Godot versions.
/// Note - `StringName::is_valid_identifier` is being phased out in favor of `is_valid_ascii_identifier()`
/// (there is also `TextServer.is_valid_identifier()` -> I tested it, `TextServer.FEATURE_UNICODE_IDENTIFIERS` seems to be enabled by default.)
// TODO: maybe encode this check in the type system so we can we-use the logic for :set and :for and |i> and the others
pub(super) fn is_valid_variable_name(s: &str) -> bool {
    if s == "_" {
        return false; // breaks godot if you do `let _ = get_meta("_")`, StringName::is_valid_identifier also rejects it
    }

    // Note - we do not need to check built-in function names like "print", it seems impossible to override them in my attempts
    if RESERVED_KEYWORDS.contains(s) {
        return false;
    }

    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false, // empty string or starts with digit/other
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// All reserved GDScript keywords (AFAIK)
static RESERVED_KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Control flow
        "if",
        "elif",
        "else",
        "for",
        "while",
        "match",
        "when",
        "break",
        "continue",
        "pass",
        "return",
        // Class/type stuff
        "class",
        "class_name",
        "extends",
        "is",
        "as",
        "self",
        "super",
        "signal",
        "func",
        "static",
        "const",
        "enum",
        "var",
        "void",
        // Misc keywords
        "breakpoint",
        "preload",
        "await",
        "yield",
        "assert",
        // Built-in constants -> technically valid GDScript, but you probably don't wanna mess with these
        "PI",
        "TAU",
        "INF",
        "NAN",
        // Literals
        "true",
        "false",
        "null",
        // Operators
        "in",
        "not",
        "and",
        "or",
    ]
    .into_iter()
    .collect()
});

pub(super) async fn show_dialog(title: &str, text: &str) {
    let mut dialog = AcceptDialog::new_alloc();
    dialog.set_title(title);

    // Add custom label so we can make it wrap automatically
    let mut label = Label::new_alloc();
    label.set_text(text);
    label.set_autowrap_mode(AutowrapMode::WORD_SMART);
    label.set_custom_minimum_size(Vector2::new(400.0, 0.0));

    // Add scroll container to prevent long text from making the dialog impossible to close
    let mut scroll = ScrollContainer::new_alloc();
    scroll.set_custom_minimum_size(Vector2::new(400.0, 300.0));
    scroll.add_child(&label);
    dialog.add_child(&scroll);

    get_root().add_child(&dialog);
    dialog
        .popup_centered_clamped_ex()
        .minsize(Vector2i::new(500, 80))
        .fallback_ratio(0.8) // <-- does not seem to work, if you feed a very long string into it it will occupy more than full screen
        .done();

    let mut dialog2 = dialog.clone();
    defer! {
        // Do not forget this, or you get a memory leak if the future is cancelled
        dialog2.queue_free();  // This is idempotent, so calling is twice is harmless
    }

    let _ = dialog
        .signals()
        .visibility_changed()
        .to_fallible_future()
        .await
        .is_ok();
}

pub(super) async fn show_prompt(title: &str, prompt: &str) -> Option<String> {
    let mut dialog = AcceptDialog::new_alloc();
    dialog.set_title(title);

    let mut line_edit = LineEdit::new_alloc();
    line_edit.set_placeholder(prompt);
    dialog.add_child(&line_edit);

    get_root().add_child(&dialog);
    dialog
        .popup_centered_clamped_ex()
        .minsize(Vector2i::new(500, 80))
        .fallback_ratio(0.8)
        .done();
    line_edit.grab_focus();

    let mut dialog2 = dialog.clone();
    defer! {
        // Do not forget this, or you get a memory leak if the future is cancelled
        dialog2.queue_free(); // This is idempotent, so calling is twice is harmless
    }

    let confirmed = dialog.signals().confirmed().to_fallible_future();
    let visibility_changed = dialog.signals().visibility_changed().to_fallible_future();
    let line_accepted = line_edit.signals().text_submitted().to_fallible_future();

    tokio::select! {
        biased; // <-- important, to ensure `line_accepted` always triggers before `confirmed` which always triggers before `visibility_changed`
        Ok((text,)) = line_accepted => Some(text.to_string()),
        Ok(_) = confirmed => Some(line_edit.get_text().to_string()),
        Ok(_) = visibility_changed => None,
        else => None, // One of the futures returned Err, so it was already freed
    }
}

/// Takes a screenshot, saves it to a png file on another thread, and returns the path it was saved to.
///
/// It will place the screenshots in `user://screenshots/`. If that folder doesn't exist, it will be created.
pub(super) async fn take_screenshot() -> EyreResult<PathBuf> {
    let mut img = get_viewport()
        .get_texture()
        .ok_or_else(|| eyre!("Failed to get viewport texture"))?
        .get_image()
        .ok_or_else(|| eyre!("Failed to get viewport texture's image"))?; // <-- fast enough (~12ms)

    let img_format = img.get_format();
    if img_format != GodotImageFormat::RGB8 {
        tracing::warn!(
            "screenshot format wasn't RGB8, but {img_format:?} - converting it... (slow)"
        );
        profile!(img.convert(GodotImageFormat::RGB8));
    }
    let (width, height, data) = (
        img.get_width(),
        img.get_height(),
        img.get_data().to_vec(), // <-- takes ~1ms
    );

    let folder = SCREENSHOTS_FOLDER.with(|folder| (**folder).clone())?; // fails if it can't create the directory
    let path = folder.join(format!("screenshot_{}.png", generate_datetime_string()));

    let result = spawn_rayon_with_result({
        let path = path.clone(); // <-- borrow checker satisfaction
        move || {
            let data_len = data.len();
            let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
                ImageBuffer::from_raw(width as u32, height as u32, data).unwrap_or_else(|| {
                    // This is a bug, should never happen
                    panic!("data size mismatch: {width}x{height}x3 != {data_len}")
                });

            // We don't use async io here since we're on another thread anyway.
            // We not only have blocking IO, but encoding to PNG can also be hard on the CPU.
            // So that's why we 100% want to run this on another thread.

            let file = File::create_new(&path)?; // Fails if file already exists (can happen if you take more than one screenshot per second)
            let mut writer = BufWriter::new(file); // Important - `image` crate expects BufWriter for performance
            img.write_to(&mut writer, ImageFormat::Png)
        }
    })
    .await
    .expect("panic while writing screenshot");

    // Now using `image` crate instead of Godot's save_png.
    // It's literally 10x faster (300ms -> 30ms)

    if let Err(err) = result {
        return Err(eyre!("Failed to save screenshot: {err}"));
    }

    Ok(path)
}

thread_local! {

    // TODO maybe use camino Utf8PathBuf here?

    /// If you access this static, it will create the `crabbyconsole` folder and return it.
    /// If it fails to create the folder, it will not try to create it again next time, it will just store the error.
    /// Because if it fails it will likely fail again the next time (probably permission issue, or wasm).
    ///
    /// Note - do not access this before Godot initializes, or it will panic.
    /// Note 2 - using Box to reduce TLS size a bit.
    /// Note 3 - ensure you store ONLY the error in `Arc`, NOT the entire thing, or we can't use `?` anymore.
    pub(super) static CRABBYCONSOLE_FOLDER: Box<Result<PathBuf, Arc<io::Error>>> = {
        return Box::new(create_folder_if_not_exists("user://crabbyconsole/"));
    };

    pub(super) static SCREENSHOTS_FOLDER: Box<Result<PathBuf, Arc<io::Error>>> = {
        return Box::new(create_folder_if_not_exists("user://crabbyconsole/screenshots/"));
    };
}

/// Creates a folder in `user://...` format if it does not exist yet.
fn create_folder_if_not_exists(godot_path: &str) -> Result<PathBuf, Arc<io::Error>> {
    assert!(is_main_thread());

    let folder = PathBuf::from(
        ProjectSettings::singleton()
            .globalize_path(godot_path)
            .to_string(),
    );

    // This will succeed even if the folder already exists.
    // It will only fail if e.g. it does not have permission to create the folder (e.g. in wasm)
    std::fs::create_dir_all(&folder)?;

    tracing::info!(?folder, "created folder `{godot_path}`");

    Ok(folder)
}

////////

/// Use this type in your `thread_local!` caches.
/// It boxes the `Cache`, since `Cache` is a huge type (300 bytes+).
/// Otherwise it will blow up because it doesn't fit in the TLS buffer on Linux.
///
/// NOTE: we use `RefCell<Box<Cache<...>>>` instead of `Box<RefCell<Cache<...>>` here.
/// At first glance it may seems inconsistent with common patterns like like `Rc<RefCell<T>>` and `Arc<Mutex<T>>`.
/// However, this is needed so we can use `with_borrow_mut()` on the `RefCell`.
/// Because if you put this in a `thread_local!`, the total type becomes `LocalKey<RefCell<Box<Cache<T>>>>`.
/// And, `with_borrow_mut()` can only be used if the type matches `LocalKey<RefCell<...>>`.
/// The `RefCell` has to be directly inside the `LocalKey`, it does not work if there is a `Box` in-between them.
pub(super) type BoxedCache<K, V> = RefCell<Box<Cache<K, V>>>;

pub struct BuildInfo {
    pub build_id: &'static str,
    pub actual_profile: &'static str,
    pub build_features: &'static str,
}
pub static BUILD_INFO: OnceLock<BuildInfo> = OnceLock::new();

#[tracing::instrument(skip_all)]
pub fn assign_build_info(bi: BuildInfo) {
    if BUILD_INFO.set(bi).is_err() {
        tracing::warn!("build info was already set");
    }
}
////////

/// Get information about the system.
/// I tried to make this more strictly-typed but it gives weird Dictionary type errors and panics.
pub fn get_sysinfo() -> Dictionary<Variant, Variant> {
    let os = Os::singleton();
    let rendering_server = RenderingServer::singleton();
    let display_server = DisplayServer::singleton();

    let memory_total_mb = os
        .get_memory_info()
        .get("physical")
        .map(|v| v.to::<i64>() as f64 / 1024.0 / 1024.0)
        .unwrap_or(0.0);

    let os_info = vdict! {
        "name" => &os.get_name(),
        "distribution" => &os.get_distribution_name(),
        "locale" => &os.get_locale(),
        "executable_path" =>& os.get_executable_path(),
    };

    let hardware_info = vdict! {
        "model_name" => &os.get_model_name(),
        "processor_name" => &os.get_processor_name(),
        "processor_count" => os.get_processor_count(),
        "memory_total_mb" => memory_total_mb,
    };

    let graphics_info = vdict! {
        "video_adapter" => &rendering_server.get_video_adapter_name(),
        "video_adapter_vendor" => &rendering_server.get_video_adapter_vendor(),
        "video_adapter_api_version" => &rendering_server.get_video_adapter_api_version(),
        "screen_count" => display_server.get_screen_count(),
        "screen_resolution" => display_server.screen_get_size(),
    };

    let engine_info = vdict! {
        "godot_runtime_version" => GdextBuild::godot_runtime_version_string(), // Godot version running the game
        "godot_static_version" => GdextBuild::godot_static_version_string(), // Godot-api level godot-rust compiles against
        "is_exported_build" => !os.has_feature("editor"),

    };

    let bi = BUILD_INFO
        .get()
        .expect("build info not set, ensure you called assign_build_info()");
    let cc_info = vdict! {
        "crate_version" => format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        "build_id" => bi.build_id,
        "build_profile" =>  bi.actual_profile,
        "build_features" =>  bi.build_features,

    };

    let features = VarArray::from_iter(active_godot_features().into_iter().map(|s| s.to_variant()));

    vdict! {
        "os" => &os_info,
        "hardware" => &hardware_info,
        "graphics" => &graphics_info,
        "engine" => &engine_info,
        "crabbyconsole" => &cc_info,
        "active_godot_features" => &features,
    }

    // TODO maybe make this strictly typed instead of a dict?
}

/// Returns all currently active Godot features.
/// NOTE - this is NOT an exhaustive list, more may get added in the future.
/// There may also be some platform-specific features not listed here.
fn active_godot_features() -> Vec<&'static str> {
    const FEATURES: &[&str] = &[
        "windows",
        "linux",
        "macos",
        "android",
        "ios",
        "web",
        "bsd",
        // ---
        "debug",
        "editor",
        "editor_hint",
        "editor_runtime",
        "embedded_in_editor",
        "template",
        "template_debug",
        "template_release",
        "release",
        "movie",
        // ---
        "double",
        "single",
        // ---
        "64",
        "32",
        // ---
        "universal",
        "x86_64",
        "x86_32",
        "x86",
        "arm64",
        "arm32",
        "armv7a",
        "armv7",
        "armv7s",
        "arm",
        "rv64",
        "riscv",
        "ppc64",
        "ppc",
        "wasm64",
        "wasm32",
        "wasm",
        "loongarch64",
        "simulator",
        // ---
        "threads",
        "nothreads",
        // ---
        "mobile",
        "pc",
        // ---
        "web_android",
        "web_ios",
        "web_linuxbsd",
        "web_macos",
        "web_windows",
        // ---
        "etc",
        "etc2",
        "s3tc",
        // ---
        "shader_baker",
        "dedicated_server",
    ];

    FEATURES
        .iter()
        .copied()
        .filter(|feature| Os::singleton().has_feature(*feature))
        .collect()
}

/// Gets the Aabb of a Node3D in world space.
/// TODO maybe expose this method to gdscript, so the user can use it in the console.
/// That way, you can draw a node's AABB but offset or transformed in some custom way
pub fn get_node3d_global_aabb(n3d: &Gd<Node3D>) -> Aabb {
    if let Ok(c) = n3d.clone().try_cast::<CollisionObject3D>() {
        let mut aabbs = vec![]; // <-- in global space

        // note - shape owners are NOT guaranteed to be contiguous, so don't iterate using 0..shape_count
        for shape_owner in c.get_shape_owners().as_slice() {
            // on the other hand, shapes within a shape owner ARE contiguous, so we can use 0..n iteration
            for shape_id in 0..c.shape_owner_get_shape_count(*shape_owner as u32) {
                let shape = c
                    .shape_owner_get_shape(*shape_owner as u32, shape_id)
                    .unwrap_or_else(|| {
                        panic!("missing shape with id {shape_id} in shape_owner {shape_owner}")
                    });

                // Note - this works in an exported game as well
                let debug_mesh = shape.get_debug_mesh().expect("no debug mesh"); // <-- TODO may be very slow for concave meshes

                let global_aabb = n3d.get_global_transform()
                    * c.shape_owner_get_transform(*shape_owner as u32)
                    * debug_mesh.get_aabb();
                aabbs.push(global_aabb)
            }
        }

        // Now reduce them together into one big AABB.
        // (we don't start with a "identity" AABB here, or it may be incorrectly included, e.g. if the node3d's origin lies outside the shape's AABB)
        aabbs
            .into_iter()
            .reduce(|left, right| left.merge(right))
            .unwrap_or_else(|| {
                tracing::warn!("CollisionObject3D has no shapes - using dummy AABB");
                Aabb::new(n3d.get_global_position(), Vector3::ONE * 0.1)
            })
    } else if let Ok(vis) = n3d.clone().try_cast::<VisualInstance3D>() {
        // If it's a VisualInstance3D, we're in luck, we can easily get the AABB like this:
        vis.get_global_transform() * vis.get_aabb()
    } else {
        // Not sure if a raycast can ever return an object that isn't a CollisionObject3D, but okay
        tracing::warn!(
            "object wasn't a CollisionObject3D nor a VisualInstance3D - using dummy AABB"
        );
        Aabb::new(n3d.get_global_position(), Vector3::ONE * 0.5)
    }
}

/// Loads a Resource on background thread using ResourceLoader.
/// TODO we can use godot::tools::try_load_threaded(path) instead of this now!
pub async fn load_threaded(path: &str) -> Result<Gd<Resource>, Report> {
    let mut loader = ResourceLoader::singleton();

    if loader.load_threaded_request(path) != godot::global::Error::OK {
        return Err(eyre!("failed to start threaded load for `{path}`"));
    }

    loop {
        match loader.load_threaded_get_status(path) {
            ThreadLoadStatus::IN_PROGRESS => {
                // Spin loop: check every frame if the resource is ready
                wait_for_next_frame().await;
            }
            ThreadLoadStatus::LOADED => break,
            e @ (ThreadLoadStatus::FAILED | ThreadLoadStatus::INVALID_RESOURCE) => {
                return Err(eyre!("threaded load failed for `{path}` ({e:?})"));
            }
            _ => unreachable!(),
        }
    }

    let resource = loader
        .load_threaded_get(path)
        .ok_or_else(|| eyre!("failed to get loaded resource at `{path}`"))?;

    Ok(resource)
}
