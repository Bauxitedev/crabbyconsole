//! This module contains the script execution context (aka the object that contains the scripts entered in the console)

use crabbyconsole_misc::util::{
    get_camera_2d, get_camera_3d, get_current_scene, get_root, get_scene_tree, get_viewport,
    project_aabb_to_2d,
};
use godot::{
    classes::{Camera2D, Camera3D, Viewport, Window},
    prelude::*,
    tools::try_get_autoload_by_name,
};
use rand::RngExt;

use crate::gd::console::{CrabConsole, util::get_cursor_world_position};

// We use no_init here to prevent being able to create a CrabConsoleScriptContext without passing a CrabConsole into it
#[derive(GodotClass, Debug)]
#[class(no_init, base=RefCounted)]
pub(crate) struct CrabConsoleScriptContext {
    base: Base<RefCounted>,

    // Note - this is a cyclic reference (CrabConsole -> CrabConsoleScriptContext -> CrabConsole)
    // But it seems fine, because CrabConsole is a Node, so NOT RefCounted, so this shouldn't cause a mem leak
    console: Gd<CrabConsole>,
}

#[godot_api]
impl CrabConsoleScriptContext {
    pub fn new(console: Gd<CrabConsole>) -> Gd<Self> {
        Gd::from_init_fn(|base| Self { base, console })
    }

    // NOTE - all of the following methods need #[func(gd_self)] and Gd<Self>, not &self,
    // otherwise it panics when you call it from the CrabConsole.
    // See https://godot-rust.github.io/docs/gdext/master/godot/prelude/attr.godot_api.html#associated-functions-and-methods
    // This has to do with re-entrancy: the commands are handled in _process, so self is already borrowed.
    // NOTE2 - this may no longer be the case after moving to CrabConsoleScriptContext

    // TODO all of these should become clap commands, so we can do...
    // - eval_blocking(":scene")
    // - eval_blocking(":win cursor")
    // - eval_blocking(":win cursor --3d")
    // - eval_blocking(":cam get") or ":cam 2d get"?
    // - eval_blocking(":node autoload foo")

    /// Get the current scene. Returns nil if there is none.
    #[func]
    fn scene() -> Variant {
        get_current_scene().to_variant()
    }

    /// Get the current scene tree. Panics if there is no scene tree or its type isn't MainLoop.
    #[func]
    fn tree() -> Gd<SceneTree> {
        get_scene_tree()
    }

    /// Get the current scene tree root. Panics if there is no scene tree or its type isn't MainLoop.
    #[func]
    fn root() -> Gd<Window> {
        get_scene_tree().get_root().expect("no root")
    }

    /// Get current viewport.
    #[func]
    fn viewport() -> Gd<Viewport> {
        // This method is needed since CrabConsoleScriptContext is no longer a Node
        // TODO - maybe we can use self.console.get_viewport() instead?
        // That way it works if the CrabConsole is in its own viewport, e.g. for VR or split-screen?
        get_viewport()
    }

    /// Gets the given autoload.
    #[func]
    fn autoload(name: String) -> Option<Gd<Node>> {
        try_get_autoload_by_name::<Node>(&name).ok()
    }

    /// Add a node to the current scene. If there is no current scene, adds it to the scene tree root instead.
    #[func]
    fn add_child(node: Gd<Node>) {
        // This method is needed since CrabConsoleScriptContext is no longer a Node
        get_current_scene()
            .unwrap_or_else(|| get_root().upcast())
            .add_child(&node);
    }

    /// Gets the node at the given `path`
    #[func]
    fn get_node(path: String) -> Gd<Node> {
        // This method is needed since CrabConsoleScriptContext is no longer a Node
        // Note - renaming the method to `node()` seems like a bad idea.
        // You generally want to allow the user to define a variable called `node` without risk of conflicts.

        // Get node relative to /root/ now, not `scene()` (since it's not present during integration testing)
        // If you want the old behavior, use scene().get_node() instead.
        get_root().get_node_as(&path)
    }

    /// Get the current 2D camera. Panics if there is none.
    #[func]
    fn cam_2d() -> Gd<Camera2D> {
        get_camera_2d().expect("no camera 2d") // TODO return Variant::nil instead of panic
    }

    /// Get the current 3D camera. Panics if there is none.
    #[func]
    fn cam_3d() -> Gd<Camera3D> {
        get_camera_3d().expect("no camera 3d") // TODO return Variant::nil instead of panic
    }

    /// Get current cursor position in 2D.
    #[func]
    fn cursor_2d() -> Vector2 {
        get_viewport().get_mouse_position()
    }

    /// Get current cursor position in 3D along the given depth. Panics if there is no Camera3D in the scene.
    #[func]
    fn cursor_3d(depth: f32) -> Vector3 {
        let cam = get_camera_3d().expect("no camera 3d"); // TODO return Variant::nil instead of panic
        get_cursor_world_position(depth, cam)
    }

    /// Shorthand notation for Vector2()
    #[func]
    fn v2(x: f32, y: f32) -> Vector2 {
        Vector2::new(x, y)
    }

    /// Shorthand notation for Vector3()
    #[func]
    fn v3(x: f32, y: f32, z: f32) -> Vector3 {
        Vector3::new(x, y, z)
    }

    /// Shorthand notation for Rect2()
    #[func]
    fn r2(p: Vector2, s: Vector2) -> Rect2 {
        Rect2::new(p, s)
    }

    /// Projects a 3D AABB to a 2D Rect2. Panics if there is no Camera3D.
    #[func]
    fn project_aabb_to_2d(aabb: Aabb) -> Rect2 {
        let cam = get_camera_3d().expect("no camera 3d"); // TODO return Variant::nil instead of panic

        project_aabb_to_2d(aabb, &cam)
    }

    /// Generates a random 2D vector in range -1..1
    #[func]
    pub fn random_vec2() -> Vector2 {
        let mut rng = rand::rng();
        let range = -1.0..1.0;
        Vector2::new(
            rng.random_range(range.clone()),
            rng.random_range(range.clone()),
        )
    }

    /// Generates a random 3D vector in range -1..1
    #[func]
    pub fn random_vec3() -> Vector3 {
        let mut rng = rand::rng();
        let range = -1.0..1.0;
        Vector3::new(
            rng.random_range(range.clone()),
            rng.random_range(range.clone()),
            rng.random_range(range),
        )
    }

    ///////////////////

    /// Call this to test if panic recovery works
    #[func]
    fn panic() {
        panic!("script context requested panic");
    }

    /// Use this to generate and evaluate clap commands dynamically.
    /// Note - if you call gdscript through this, it will probably fail due to re-entrancy.
    #[func]
    fn eval_blocking(&self, expression: String) {
        CrabConsole::eval_blocking(self.console.clone(), expression); // fast clone
    }
}
