//! Misc useful methods/types

use std::{
    fmt::{self, Debug},
    fs::OpenOptions,
    io,
    path::Path,
    sync::LazyLock,
};

use clap::error::ErrorKind;
use flume::Receiver;
use godot::{
    classes::{Camera2D, Camera3D, Engine, FileAccess, Os, ResourceLoader, Viewport, Window},
    global::Key,
    obj::Singleton as _,
    prelude::*,
};
use indexmap::{IndexMap, IndexSet};
use sysinfo::{DiskUsage, Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use time::OffsetDateTime;

//////////////////////////

// append statistics (use IndexMap to force consistent order)

#[derive(Debug)]
pub enum AValue {
    Float(f32),
    Float64(f64),
    Bool(bool),
    Int(i32),
    Text(String),
}

impl fmt::Display for AValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Float(v) => write!(f, "{v}"),
            Self::Float64(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Text(v) => write!(f, "{v}"),
        }
    }
}

impl From<f32> for AValue {
    fn from(v: f32) -> Self {
        Self::Float(v)
    }
}
impl From<f64> for AValue {
    fn from(v: f64) -> Self {
        Self::Float64(v)
    }
}
impl From<bool> for AValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

pub fn generate_datetime_string() -> String {
    // The offset can be missing in e.g. sandboxed environments.
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());

    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )

    //I think I prefer chrono's api here...
    //Update - chrono just got deprecated, so let's not use it, use jiff or time instead
}

/// All `append()` statements will write to the same datetime.
/// This is the time `append()` was called for the first time during this execution of the app.
/// (So NOT the time the program was started.)
static APPEND_DATETIME: LazyLock<String> = LazyLock::new(generate_datetime_string);

pub fn append<F>(map: &IndexMap<&str, AValue>, path_fn: F) -> io::Result<()>
where
    F: Fn(&str) -> String,
{
    let path = path_fn(&APPEND_DATETIME);

    tracing::info!("starting append to {path}...");

    let write_headers = !Path::new(&path).exists() || std::fs::metadata(&path)?.len() == 0;

    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut wtr = csv::Writer::from_writer(file);

    let keys: Vec<&str> = map.keys().copied().collect();
    let values: Vec<String> = keys.iter().map(|k| map[k].to_string()).collect();

    if write_headers {
        wtr.write_record(&keys)?;
    }
    wtr.write_record(&values)?;
    wtr.flush()?;

    tracing::info!("appended to {path}");
    Ok(())
}

// BBCode stuff //

pub fn escape_bbcode(text: &str) -> String {
    // This is all we need.
    // See https://docs.godotengine.org/en/4.6/tutorials/ui/bbcode_in_richtextlabel.html#doc-bbcode-in-richtextlabel-handling-user-input-safely
    text.replace('[', "[lb]")
}

// Unlike Godot's is_equal_approx, this one work across threads
pub fn approx_eq(a: f64, b: f64, epsilon: f64) -> bool {
    (a - b).abs() < epsilon
}

// Node stuff //

/// Gets the scene tree. Panics if there is no main loop or its type isn't `SceneTree`.
pub fn get_scene_tree() -> Gd<SceneTree> {
    Engine::singleton()
        .get_main_loop()
        .expect("no main loop")
        .cast::<SceneTree>() // <-- assumes the main loop is a `SceneTree`, will panic otherwise
}

/// Gets the scene tree root. Panics if there is no main loop or its type isn't `SceneTree`.
pub fn get_root() -> Gd<Window> {
    get_scene_tree().get_root().expect("no root")
}

/// Gets the current scene. Panics if there is no main loop or its type isn't `SceneTree`.
pub fn get_current_scene() -> Option<Gd<Node>> {
    get_scene_tree().get_current_scene()
}

/// Get current viewport.
pub fn get_viewport() -> Gd<Viewport> {
    get_root().upcast()
}

/// Get current camera 2D
pub fn get_camera_2d() -> Option<Gd<Camera2D>> {
    get_viewport().get_camera_2d()
}

/// Get current camera 3D. This may go wrong if you have multiple viewports (e.g. a split-screen multiplayer game).
pub fn get_camera_3d() -> Option<Gd<Camera3D>> {
    get_viewport().get_camera_3d()
}

/// Block on the channel and wait until at least 1 message arrives.
///
/// If there are multiple messages, only the latest one is retained, the rest is dropped.
/// This is meant to be called in a tight loop, where processing every message takes a long time.
/// That means messages can accumulate in the meantime, so we only retain the latest one, since the others are stale.
///
/// Always returns at least one element (it will block if we don't have one yet)
/// ...unless all senders are dropped and no messages are left, in which case we return None.
pub fn wait_and_retain_latest_message<T>(rx: &Receiver<T>) -> Option<T> {
    let peeked = rx.recv().ok()?; // return None if recv() returned Err

    let mut msgs: Vec<T> = rx.try_iter().collect();
    msgs.insert(0, peeked);
    Some(msgs.pop().unwrap()) // Safe unwrap, since msgs already contains at least 1 element
}

/// Async variant of the above.
///
/// Note - I added a `Send` bound on `T` solely to satisfy clippy, you can remove it later if needed.
pub async fn wait_and_retain_latest_message_async<T: Send>(rx: &Receiver<T>) -> Option<T> {
    let peeked = rx.recv_async().await.ok()?; // return None if recv() returned Err

    let mut msgs: Vec<T> = rx.try_iter().collect();
    msgs.insert(0, peeked);
    Some(msgs.pop().unwrap()) // Safe unwrap, since msgs already contains at least 1 element
}

/// If the error type is `DisplayHelp`, `DisplayHelpOnMissingArgumentOrSubcommand`, or `DisplayVersion`,
/// do not print an error, instead just print the help
pub fn is_fake_clap_error(err: &clap::Error) -> bool {
    #[allow(clippy::match_like_matches_macro)]
    match err.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        | ErrorKind::DisplayVersion => true,
        _ => false,
    }
}

/// Returns the amount of ram usage of the current process.
/// To quote the documentation from the `sysinfo` crate:
///
/// "This method returns the size of the resident set, that is,
/// the amount of memory that the process allocated and which is
/// currently mapped in physical RAM. It does not include memory
/// that is swapped out, or, in some operating systems, that has
/// been allocated but never used.
/// Thus, it represents exactly the amount of physical RAM that
/// the process is using at the present time, but it might not be
/// a good indicator of the total memory that the process will be
/// using over its lifetime.
/// For that purpose, you can try and use `virtual_memory`."
///
/// Note this is slow on Windows (5ms+) so use sparingly.
pub fn get_process_ram_bytes() -> u64 {
    let mut sys = System::new();
    let pid = Pid::from(std::process::id() as usize);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]), // Only update current process
        false,
        ProcessRefreshKind::nothing().with_memory(),
    ); // Note - refresh_processes_specifics is kinda slow

    sys.process(pid).unwrap().memory()
}

/// Returns the total amount of bytes written/read since process start.
///
/// You can delta the result to get the current write/read speed.
///
/// Note - Files might be cached in memory by your OS, meaning that reading/writing them might not increase the `read_bytes/written_bytes` value
pub fn get_process_total_disk_usage_bytes() -> DiskUsage {
    let mut sys = System::new();
    let pid = Pid::from(std::process::id() as usize);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]), // Only update current process
        false,
        ProcessRefreshKind::nothing().with_disk_usage(),
    ); // Note - refresh_processes_specifics is kinda slow

    sys.process(pid).unwrap().disk_usage()
}

//////////

/// Truncates `s` to at most `len` characters, appending ellipsis if truncation occurred.
///
/// `len` is the total length of the resulting string (including the ellipsis).
pub fn truncate_with_ellipsis(s: &str, len: usize) -> String {
    const ELLIPSIS: &str = "...";

    let char_count = s.chars().count();
    if char_count <= len {
        return s.to_string();
    }

    let ellipsis_len = ELLIPSIS.chars().count();

    // If max_len is too small to even fit the ellipsis, just return ellipsis only.
    if len <= ellipsis_len {
        return ELLIPSIS.chars().take(len).collect();
    }

    let truncated: String = s.chars().take(len - ellipsis_len).collect();
    format!("{truncated}{ELLIPSIS}")
}

///////////////

/// Projects a 3D AABB to a 2D Rect2.
pub fn project_aabb_to_2d(aabb: Aabb, camera: &Gd<Camera3D>) -> Rect2 {
    let mut rect: Option<Rect2> = None;

    for i in 0..8 {
        let corner = aabb.get_corner(i);

        // Skip corners behind the camera
        if camera.is_position_behind(corner) {
            continue;
        }

        let screen_pos = camera.unproject_position(corner);

        rect = Some(match rect {
            None => Rect2::new(screen_pos, Vector2::ZERO),
            Some(r) => r.expand(screen_pos),
        });
    }

    rect.unwrap_or_default()
}

///////////////

/// Returns a sorted list of all resource paths under `res://`, recursively.
///
/// Skips hidden directories (prefixed with `.`) and directories containing a `.gdignore` file.
///
/// This is also useful for auto-complete!
pub fn get_all_resources() -> Vec<String> {
    let path = "res://";

    fn scan_dir(path: &str, result: &mut Vec<String>) {
        let mut loader = ResourceLoader::singleton();

        // skip directories that contain .gdignore
        let ignore_marker = format!("{path}.gdignore");
        if FileAccess::file_exists(&ignore_marker) {
            return;
        }

        for entry in loader.list_directory(path).as_slice() {
            let entry = entry.to_string();
            if entry.starts_with('.') {
                continue;
            }
            if entry.ends_with('/') {
                scan_dir(&format!("{path}{entry}"), result);
            } else {
                result.push(format!("{path}{entry}"));
            }
        }
    }

    let mut result = Vec::new();
    scan_dir(path, &mut result);
    result.sort(); // Important, since list_directory returns in non-deterministic order
    result
}

/// This is needed because `Key::all_constants()` contains a lot of garbage we need to filter out.
///
/// This is super fast, by the way, no need to cache it (<1ms).
pub fn build_key_name_map() -> IndexMap<String, Key> {
    let os = Os::singleton();

    let mut duplicate_checker = IndexSet::<String>::default();

    Key::all_constants()
        .iter()
        .filter_map(|constant| {
            let key = constant.value();

            // Note - os.get_keycode_string(Key::SPECIAL) will spam Godot errors about invalid UTF-8, so skip it early
            if key == Key::SPECIAL {
                return None;
            }

            // store in lowercase, so we can look them up in a case-insensitive way
            let name = os.get_keycode_string(key).to_string().to_lowercase();
            let valid = !name.is_empty() // the first one is always empty
                && name.chars().all(|c| c.is_ascii_alphanumeric()) // some names contain spaces (notably all keypad binds), so cannot be used in :key bind
                && key != Key::UNKNOWN;

            if valid {
                let inserted = duplicate_checker.insert(name.clone());
                if !inserted {
                    tracing::warn!("duplicate key {name} ({key:?})");
                }
            }

            valid.then_some((name, key))
        })
        .collect()
}
