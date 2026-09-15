# GDScript functions

This is a list of built-in GDScript functions you can use while evaluating a GDScript command.

| Name | Description | Return type |
| --- | --- | --- |
| `scene()` | Gets the current scene | `Node` |
| `tree()` | Gets the current scene tree | `SceneTree` |
| `root()` | Gets the root of the current scene tree| `Window` |
| `viewport()` | Get current viewport | `Viewport` |
| `autoload(name: string)` | Gets the autoload with name `name` | `Node` |
| `cam_2d()` | Gets the current 2D camera | `Camera2D` |
| `cam_3d()` | Gets the current 3D camera | `Camera3D` |
| <hr> | <hr> | <hr> |
| `add_child(node: Node)` | Add a node to the current scene | |
| `get_node(path: String)` | Gets the node at the given `path` | |
| `cursor_2d()` | Gets the current cursor position in 2D | `Vector2` |
| `cursor_3d(depth: float)` | Gets the current cursor position in 3D, at distance `depth` from the 3D camera | `Vector3` |
| `v2(x: float, y: float)` | Shorter way to write `Vector2(x, y)` | `Vector2` |
| `v3(x: float, y: float, z: float)` | Shorter way to write `Vector3(x, y, z)` | `Vector3` |
| `r2(p: Vector2, s: Vector2)` | Shorter way to write `Rect2(p, s)` | `Rect2` |
| `project_aabb_to_2d(aabb: Aabb)` |  Projects a 3D AABB to a 2D Rect2 | `Rect2` |
| `random_vec2()` |  Generates a random 2D vector in range -1..1 | `Vector2` |
| `random_vec3()` |  Generates a random 3D vector in range -1..1 | `Vector3` |
