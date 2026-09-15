use std::{
    any::type_name,
    f32::consts::PI,
    time::{Duration, Instant},
};

use color_eyre::{
    Report,
    eyre::{ensure, eyre},
};
use crabbyconsole_clap::draw::{
    DrawAction, DrawAction2D, DrawAction3D, HyperrectangleArgs, HypersphereArgs, LineArgs,
    TextAlignment,
};
use crabbyconsole_misc::{FutureTracyExt as _, gd::async_node::AsyncGd, util::get_camera_3d};
use godot::{
    classes::{
        CanvasItem, Font, INode2D, ImmediateMesh, MeshInstance3D, Node2D, mesh::PrimitiveType,
    },
    global::HorizontalAlignment,
    prelude::*,
};
use rand::RngExt;

use crate::gd::console::{
    CrabConsole, JobExpressionInner, eval::ClapSubAction, util::get_node3d_global_aabb,
};

pub enum DebugShapeKind2D {
    Line {
        // TODO allow storing InstanceID here IN ADDITION TO Vector2?
        // (we need both in case it gets freed, so we can remember its position from the previous frame)
        from: Vector2,
        to: Vector2,
        width: f32,
    },

    Rectangle {
        rect: Rect2,
        width: f32,
    },

    Circle {
        pos: Vector2,
        radius: f32,
    },

    Text {
        pos: Vector2,
        align: TextAlignment,
        font: Gd<Font>,
        texts: Vec<String>,
    },
}

pub enum DebugShapeKind3D {
    Line { from: Vector3, to: Vector3 },
}

struct DebugShape<T> {
    kind: T,

    color: Color,
    expiry_time: Instant,
}

impl DebugShape<DebugShapeKind2D> {
    fn draw(&self, mut base: Gd<Node2D>) {
        match &self.kind {
            DebugShapeKind2D::Line { from, to, width } => {
                base.draw_line_ex(*from, *to, self.color)
                    .width(*width)
                    .antialiased(true) // <-- TODO ensure this isn't slow, in that case make it togglable
                    .done();
            }
            DebugShapeKind2D::Rectangle { rect, width } => {
                base.draw_rect_ex(*rect, self.color)
                    .filled(false)
                    .width(*width)
                    .done();
            }
            DebugShapeKind2D::Circle { pos, radius } => {
                base.draw_circle_ex(*pos, *radius, self.color)
                    .antialiased(true)
                    .filled(true)
                    .done();
            }
            DebugShapeKind2D::Text {
                pos,
                align,
                font,
                texts,
            } => {
                // The other alignment modes don't work so don't bother, unless you specify a width manually.
                // See https://github.com/godotengine/godot/issues/80163
                let alignment = HorizontalAlignment::LEFT;

                let font_size = 16;
                let line_height = font.get_height_ex().font_size(font_size).done() + 4.; // small margin

                // Need this, since text by default draws at the baseline, so not top-aligned
                let ascent = font.get_ascent_ex().font_size(font_size).done();
                let baseline_y = pos.y + ascent;

                for (line, text) in texts.iter().enumerate() {
                    let text_size = font
                        .get_string_size_ex(text)
                        .font_size(font_size)
                        .alignment(alignment)
                        .done();

                    // Offset it manually
                    let pos = match align {
                        TextAlignment::Left => Vector2::new(pos.x, baseline_y),
                        TextAlignment::Center => {
                            Vector2::new(pos.x - text_size.x / 2.0, baseline_y)
                        }
                        TextAlignment::Right => Vector2::new(pos.x - text_size.x, baseline_y),
                    } + Vector2::new(0., line as f32 * line_height);

                    // Draw text outline first
                    base.draw_string_outline_ex(font, pos, text)
                        .font_size(16)
                        .alignment(alignment)
                        .modulate(Color::BLACK)
                        .done();

                    // Then draw text itself
                    base.draw_string_ex(font, pos, text)
                        .font_size(16)
                        .alignment(alignment)
                        .modulate(self.color)
                        .done();
                }
            }
        }
    }
}

impl DebugShape<DebugShapeKind3D> {
    fn draw(&self, mut mesh: Gd<ImmediateMesh>) {
        let color = self.color;
        match self.kind {
            DebugShapeKind3D::Line { from, to } => {
                mesh.surface_set_color(color);
                mesh.surface_add_vertex(from);
                mesh.surface_add_vertex(to);
            }
        }
    }
}
/// The thing that actually draws the shapes
#[derive(GodotClass)]
#[class(base=Node2D)]
pub(crate) struct CrabConsoleDebugDrawer {
    base: Base<Node2D>,
    drawer3d_mesh: OnReady<Gd<ImmediateMesh>>,

    shapes: Vec<DebugShape<DebugShapeKind2D>>,
    shapes_3d: Vec<DebugShape<DebugShapeKind3D>>,
}

#[godot_api]
impl INode2D for CrabConsoleDebugDrawer {
    fn init(base: Base<Node2D>) -> Self {
        Self {
            base,
            drawer3d_mesh: OnReady::from_base_fn(|base| {
                let drawer3d = base.get_node_as::<MeshInstance3D>("%DebugDrawer3D");
                drawer3d
                    .get_mesh()
                    .expect("DebugDrawer3D has no mesh")
                    .cast() // Cast Gd<Mesh> -> Gd<ImmediateMesh>
            }),
            shapes: Vec::new(),
            shapes_3d: Vec::new(),
        }
    }

    fn process(&mut self, _delta: f64) {
        // Redraw every frame to ensure shapes expire
        // TODO limit to ~60 hz maybe?
        self.base_mut().queue_redraw();
    }

    fn draw(&mut self) {
        let base = self.base().clone(); // fast clone

        // Draw 2d
        for shape in &self.shapes {
            shape.draw(base.clone()); // fast clone
        }

        // Draw 3d
        self.draw_3d();

        // Delete expired shapes AFTER drawing them, to ensure new shapes are drawn at least once
        let now = Instant::now();
        self.shapes.retain(|shape| shape.expiry_time >= now);
        self.shapes_3d.retain(|shape| shape.expiry_time >= now);
    }
}

#[godot_api]
impl CrabConsoleDebugDrawer {
    pub(crate) fn draw_line_2d(
        &mut self,
        from: Vector2,
        to: Vector2,
        color: Color,
        width: f32,
        time: f32,
    ) {
        self.draw_shape_2d(DebugShapeKind2D::Line { from, to, width }, color, time);
    }

    pub(crate) fn draw_rect_2d(&mut self, rect: Rect2, color: Color, width: f32, time: f32) {
        self.draw_shape_2d(DebugShapeKind2D::Rectangle { rect, width }, color, time);
    }

    pub(crate) fn draw_circle_2d(&mut self, pos: Vector2, color: Color, radius: f32, time: f32) {
        self.draw_shape_2d(DebugShapeKind2D::Circle { pos, radius }, color, time);
    }

    pub(crate) fn draw_text_2d(
        &mut self,
        pos: Vector2,
        color: Color,
        align: TextAlignment,
        font: Gd<Font>,
        texts: Vec<String>,
        time: f32,
    ) {
        self.draw_shape_2d(
            DebugShapeKind2D::Text {
                pos,
                align,
                font,
                texts,
            },
            color,
            time,
        );
    }

    ////////////////

    pub(crate) fn draw_line_3d(&mut self, from: Vector3, to: Vector3, color: Color, time: f32) {
        self.draw_shape_3d(DebugShapeKind3D::Line { from, to }, color, time);
    }

    pub(crate) fn draw_rect_3d(&mut self, aabb: Aabb, color: Color, time: f32) {
        let edges = aabb_edges(aabb);

        for (from, to) in edges {
            let (from, to) = (from, to);
            self.draw_line_3d(from, to, color, time);
        }
    }

    pub(crate) fn draw_sphere_3d(
        &mut self,
        pos: Vector3,
        color: Color,
        radius: f32,
        resolution: usize,
        time: f32,
    ) {
        let edges = sphere_edges(radius, resolution);

        let mut rng = rand::rng();

        for (from, to) in edges {
            let (from, to) = (from + pos, to + pos);

            // cool effect just because it was easy to implement
            let time = if time > 0.01 {
                time * rng.random_range(0.8..1.2)
            } else {
                time
            };
            self.draw_line_3d(from, to, color, time);
        }
    }

    pub(crate) fn draw_transform_3d(&mut self, transform: Transform3D, scale: f32, time: f32) {
        let x = transform.basis.col_a();
        let y = transform.basis.col_b();
        let z = transform.basis.col_c();
        let origin = transform.origin;

        let mut draw_axis = |endpoint, color| {
            self.draw_line_3d(origin, origin + endpoint, color, time);
        };

        draw_axis(x * scale, Color::RED);
        draw_axis(y * scale, Color::GREEN);
        draw_axis(z * scale, Color::BLUE);
    }

    ////////////////

    pub(crate) fn draw_shape_2d(&mut self, kind: DebugShapeKind2D, color: Color, time: f32) {
        self.shapes.push(DebugShape {
            kind,
            color,
            expiry_time: Instant::now() + Duration::from_secs_f32(time),
        });
        self.base_mut().queue_redraw(); // <-- may be unneeded now
    }

    pub(crate) fn draw_shape_3d(&mut self, kind: DebugShapeKind3D, color: Color, time: f32) {
        self.shapes_3d.push(DebugShape {
            kind,
            color,
            expiry_time: Instant::now() + Duration::from_secs_f32(time),
        });
    }

    /// Clear all 2D shapes and queue redraw. Returns amount of shapes that were removed.
    pub(crate) fn clear_shapes_2d(&mut self) -> usize {
        let shape_count = self.shapes.len();
        self.shapes.clear();
        self.base_mut().queue_redraw();

        shape_count
    }

    /// Clear all 3D shapes and queue redraw. Returns amount of shapes that were removed.
    pub(crate) fn clear_shapes_3d(&mut self) -> usize {
        let shape_count = self.shapes_3d.len();
        self.shapes_3d.clear();
        self.base_mut().queue_redraw();

        shape_count
    }

    fn draw_3d(&self) {
        let mut mesh = self.drawer3d_mesh.clone(); // fast clone
        mesh.clear_surfaces();

        if self.shapes_3d.is_empty() {
            return; // important, since empty surfaces will throw errors
        }

        mesh.surface_begin_ex(PrimitiveType::LINES).done();

        for shape in &self.shapes_3d {
            shape.draw(mesh.clone()); // fast clone
        }
        mesh.surface_end();
    }
}

impl ClapSubAction for DrawAction {
    async fn handle(
        self,
        console: AsyncGd<CrabConsole>,
    ) -> Result<Variant, color_eyre::eyre::Error> {
        match self {
            DrawAction::TwoD(action) => {
                let mut debug_drawer = console.bind().nodes.debug_drawer.clone(); // fast clone
                match action {
                    DrawAction2D::Line(LineArgs {
                        time,
                        color,
                        from_to,
                    }) => {
                        let result = &eval_draw_command(from_to, console.clone()).await?;
                        let result_array = try_to_relaxed::<AnyArray>(result)?; // <-- use AnyArray, not VarArray

                        let windows = resolve_array_to_pairs(result_array, |i, p| {
                            try_all_to_vec2(&p, Some(i))
                        })?;
                        // Draw all pairs of lines
                        for [a, b] in windows {
                            debug_drawer.bind_mut().draw_line_2d(a, b, color, 1.0, time);
                        }

                        Ok(Variant::nil())
                    }

                    DrawAction2D::Rect(HyperrectangleArgs { time, color, rect }) => {
                        let result_rect2 = try_to_relaxed::<Rect2>(
                            &eval_draw_command(rect, console.clone()).await?,
                        )?;

                        debug_drawer
                            .bind_mut()
                            .draw_rect_2d(result_rect2, color, 1.0, time);

                        Ok(Variant::nil())
                    }

                    DrawAction2D::Circle(HypersphereArgs {
                        time,
                        color,
                        radius,
                        pos,
                    }) => {
                        let result = &eval_draw_command(pos, console.clone()).await?;
                        let result_vec2 = try_all_to_vec2(result, None)?;

                        debug_drawer
                            .bind_mut()
                            .draw_circle_2d(result_vec2, color, radius, time);

                        Ok(Variant::nil())
                    }

                    DrawAction2D::Text {
                        time,
                        color,
                        align,
                        pos_text,
                    } => {
                        let result = &eval_draw_command(pos_text, console.clone()).await?;
                        let (pos, texts) = try_all_to_pos_text(result)?;

                        let font_name = "normal_font"; // other options include "bold_font", "italics_font", etc
                        let font = console.bind().nodes.console_history.get_theme_font(font_name).unwrap_or_else(||
                            panic!("ConsoleHistory node has no font assigned to its theme named `{font_name}`"));

                        debug_drawer
                            .bind_mut()
                            .draw_text_2d(pos, color, align, font, texts, time);

                        Ok(Variant::nil())
                    }

                    DrawAction2D::Clear => {
                        let n = debug_drawer.bind_mut().clear_shapes_2d();
                        Ok(Variant::from(format!("Cleared {n} shapes.")))
                    }
                }
            }

            DrawAction::ThreeD(action) => {
                let mut debug_drawer = console.bind().nodes.debug_drawer.clone(); // fast clone
                match action {
                    DrawAction3D::Line(LineArgs {
                        time,
                        color,
                        from_to,
                    }) => {
                        let result = &eval_draw_command(from_to, console.clone()).await?;
                        let result_array = try_to_relaxed::<AnyArray>(result)?;

                        let windows = resolve_array_to_pairs(result_array, |i, p| {
                            try_all_to_vec3(&p, Some(i))
                        })?;
                        // Draw all pairs of lines
                        for [a, b] in windows {
                            debug_drawer.bind_mut().draw_line_3d(a, b, color, time);
                        }

                        Ok(Variant::nil())
                    }

                    DrawAction3D::Aabb(HyperrectangleArgs { time, color, rect }) => {
                        let result = &eval_draw_command(rect, console.clone()).await?;
                        let aabb = try_all_to_aabb(result)?;

                        debug_drawer.bind_mut().draw_rect_3d(aabb, color, time);

                        Ok(Variant::nil())
                    }

                    DrawAction3D::Sphere(HypersphereArgs {
                        time,
                        color,
                        radius,
                        pos,
                    }) => {
                        let result = &eval_draw_command(pos, console.clone()).await?;
                        let result_array = try_to_relaxed::<AnyArray>(result)?;

                        for (i, var) in result_array.iter_shared().enumerate() {
                            let pos = try_all_to_vec3(&var, Some(i))?;
                            // TODO it gets the position of the Node, but not its orientation...

                            // Resolution 12 = 276 edges to draw, can become laggy....
                            debug_drawer
                                .bind_mut()
                                .draw_sphere_3d(pos, color, radius, 12, time); // res 16 is too high, it lags
                        }

                        Ok(Variant::nil())
                    }

                    DrawAction3D::Transform {
                        time,
                        scale,
                        transform3d,
                    } => {
                        let result = &eval_draw_command(transform3d, console.clone()).await?;
                        let result_transform3d = try_all_to_transform3d(result)?;

                        debug_drawer
                            .bind_mut()
                            .draw_transform_3d(result_transform3d, scale, time);

                        Ok(Variant::nil())
                    }

                    DrawAction3D::Clear => {
                        let n = debug_drawer.bind_mut().clear_shapes_3d();
                        Ok(Variant::from(format!("Cleared {n} shapes.")))
                    }
                }
            }
        }
    }
}

async fn eval_draw_command(
    expression: Vec<String>,
    console: AsyncGd<CrabConsole>,
) -> Result<Variant, Report> {
    let expression = expression.join(" ");

    let result = console
        .eval_job_without_channel(JobExpressionInner::String(expression.into()))
        .with_tracy_non_continuous_frame("eval_job_draw")
        .await?;

    Ok(result)
}

fn try_to_relaxed<T: FromGodot>(result: &Variant) -> Result<T, Report> {
    let result_type = result.get_type();

    result.try_to_relaxed::<T>().map_err(|_| {
        eyre!(
            "expected expression to return a {}, \
            but got {result_type:?} instead",
            type_name::<T>()
        )
    })
}

/// Tries to convert a `Variant` to `Vector2`, `Vector3`, `CanvasItem` and `Node3D` in that order.
/// If the result is a 3D node, projects its global position to 2D.
/// `index` is only used to give better error messages to the user, in case we're dealing with an array.
fn try_all_to_vec2(var: &Variant, index: Option<usize>) -> Result<Vector2, Report> {
    let result_type = var.get_type();

    // First check if instance is valid, to prevent panics/segfaults later.
    // Not sure if this is working, but crashes/panics seem reduced now.
    if let Ok(obj) = try_to_relaxed::<Gd<Object>>(var) {
        ensure!(
            obj.is_instance_valid(),
            "cannot get 2D position of a freed Object"
        );
    }

    // Do this lazily, so it doesn't panic in the 2d case.
    let cam3d = || get_camera_3d().expect("no camera 3d"); // TODO handle this more gracefully

    if let Ok(r) = try_to_relaxed::<Vector2>(var) {
        return Ok(r);
    } else if let Ok(vec3) = try_to_relaxed::<Vector3>(var) {
        // Convert global 3D position to 2D in screen space
        return Ok(cam3d().unproject_position(vec3));
    } else if let Ok(item) = try_to_relaxed::<Gd<CanvasItem>>(var) {
        // Note: Node2D and Control both inherit CanvasItem
        // Get global position in screen space, accounting for Camera2D
        return Ok(item.get_global_transform_with_canvas().origin);
    } else if let Ok(n3d) = try_to_relaxed::<Gd<Node3D>>(var) {
        // Convert global 3D position to 2D in screen space
        return Ok(cam3d().unproject_position(n3d.get_global_position()));
    }

    // All conversions failed, so bail out
    if let Some(i) = index {
        return Err(eyre!(
            "expected expression to return an array of Vector2, Vector3, CanvasItem or Node3D, \
            but got an array element of {result_type:?} instead (at position {i})",
        ));
    }
    Err(eyre!(
        "expected expression to return a \
        Vector2, Vector3, CanvasItem or Node3D, \
        but got {result_type:?} instead",
    ))
}

/// Same idea as `try_all_to_vec2` except for 3D.
fn try_all_to_vec3(var: &Variant, index: Option<usize>) -> Result<Vector3, Report> {
    let result_type = var.get_type();

    // First check if instance is valid, to prevent panics/segfaults later.
    // Not sure if this is working, but crashes/panics seem reduced now.
    if let Ok(obj) = try_to_relaxed::<Gd<Object>>(var) {
        ensure!(
            obj.is_instance_valid(),
            "cannot get 3D position of a freed Object"
        );
    }

    let z_project_dist = 3.0; // subject to tweaking

    // Do this lazily, so it doesn't panic in the 2d case.
    let cam3d = || get_camera_3d().expect("no camera 3d"); // TODO handle this more gracefully

    if let Ok(r) = try_to_relaxed::<Vector3>(var) {
        return Ok(r);
    } else if let Ok(vec2) = try_to_relaxed::<Vector2>(var) {
        // Project 2D screen space position to 3D
        return Ok(cam3d().project_position(vec2, z_project_dist));
    } else if let Ok(item) = try_to_relaxed::<Gd<CanvasItem>>(var) {
        // Note: Node2D and Control both inherit CanvasItem
        // Get global position in screen space, accounting for Camera2D, and project to 3D
        return Ok(cam3d().project_position(
            item.get_global_transform_with_canvas().origin,
            z_project_dist,
        ));
    } else if let Ok(n3d) = try_to_relaxed::<Gd<Node3D>>(var) {
        return Ok(n3d.get_global_position());
    }

    // All conversions failed, so bail out
    if let Some(i) = index {
        return Err(eyre!(
            "expected expression to return an array of Vector2, Vector3, CanvasItem or Node3D, \
            but got an array element of {result_type:?} instead (at position {i})",
        ));
    }
    Err(eyre!(
        "expected expression to return a \
        Vector2, Vector3, CanvasItem or Node3D, \
        but got {result_type:?} instead",
    ))
}

fn try_all_to_aabb(var: &Variant) -> Result<Aabb, Report> {
    let result_type = var.get_type();

    // First check if instance is valid, to prevent panics/segfaults later.
    if let Ok(obj) = try_to_relaxed::<Gd<Object>>(var) {
        ensure!(obj.is_instance_valid(), "cannot get AABB of a freed Object");
    }

    if let Ok(aabb) = try_to_relaxed::<Aabb>(var) {
        return Ok(aabb);
    } else if let Ok(n3d) = try_to_relaxed::<Gd<Node3D>>(var) {
        return Ok(get_node3d_global_aabb(&n3d));
    }

    Err(eyre!(
        "expected expression to return a \
        Aabb or a Node3D, \
        but got {result_type:?} instead",
    ))
}

fn try_all_to_transform3d(var: &Variant) -> Result<Transform3D, Report> {
    let result_type = var.get_type();

    // First check if instance is valid, to prevent panics/segfaults later.
    if let Ok(obj) = try_to_relaxed::<Gd<Object>>(var) {
        ensure!(
            obj.is_instance_valid(),
            "cannot get Transform3D of a freed Object"
        );
    }

    if let Ok(aabb) = try_to_relaxed::<Transform3D>(var) {
        return Ok(aabb);
    } else if let Ok(n3d) = try_to_relaxed::<Gd<Node3D>>(var) {
        return Ok(n3d.get_global_transform());
    }

    Err(eyre!(
        "expected expression to return a \
        Transform3D or a Node3D, \
        but got {result_type:?} instead",
    ))
}

fn try_all_to_pos_text(var: &Variant) -> Result<(Vector2, Vec<String>), Report> {
    // We expect a VarArray of exactly two elements: [pos, text]
    let array = try_to_relaxed::<VarArray>(var)?; // TODO in the future switch to AnyArray?
    let array_vec = array.iter_shared().collect::<Vec<_>>();
    let [a, b] = array_vec.as_slice() else {
        return Err(eyre!(
            "expected an array of exactly two elements, instead got {}",
            array_vec.len()
        ));
    };

    let pos = try_all_to_vec2(a, Some(0))?;

    // If `b` is an array, stringify each element individually.
    // Note - use AnyArray here, not VarArray, or it will miss some arrays like node.get_children()
    let texts = if let Ok(texts) = try_to_relaxed::<AnyArray>(b).map(|texts| {
        texts
            .iter_shared()
            .map(|var| var.stringify().to_string())
            .collect::<Vec<_>>()
    }) {
        texts
    } else {
        // Else, stringify it as a whole
        vec![b.stringify().to_string()]
    };

    Ok((pos, texts))
}

fn resolve_array_to_pairs<T>(
    result_array: AnyArray,
    f: impl Fn(usize, Variant) -> Result<T, Report>,
) -> Result<Vec<[T; 2]>, Report>
where
    T: Clone,
{
    if result_array.len() < 2 {
        return Err(eyre!(
            "expected array of at least 2 elements, but got {} instead",
            result_array.len()
        ));
    }

    // Collect all results from the array into a Vec<VectorX>, throwing a type error if conversion failed for an element
    let positions: Vec<_> = result_array
        .iter_shared()
        .enumerate()
        .map(|(i, p)| f(i, p)) // |(i, p)| try_all_to_vec2(&p, Some(i))
        .collect::<Result<_, _>>()?;

    Ok(positions.array_windows::<2>().cloned().collect::<Vec<_>>())
}

pub fn sphere_edges(r: f32, n: usize) -> Vec<(Vector3, Vector3)> {
    let mut edges = vec![];

    let p = |lat: f32, lon: f32| {
        Vector3::new(
            r * lat.cos() * lon.cos(),
            r * lat.sin(),
            r * lat.cos() * lon.sin(),
        )
    };

    for i in 0..n {
        let lon = 2.0 * PI * i as f32 / n as f32;

        for j in 0..n {
            let a = PI * j as f32 / n as f32 - PI / 2.0;
            let b = PI * (j + 1) as f32 / n as f32 - PI / 2.0;
            edges.push((p(a, lon), p(b, lon)));
        }
    }

    for j in 1..n {
        let lat = PI * j as f32 / n as f32 - PI / 2.0;

        for i in 0..n {
            let a = 2.0 * PI * i as f32 / n as f32;
            let b = 2.0 * PI * (i + 1) as f32 / n as f32;
            edges.push((p(lat, a), p(lat, b)));
        }
    }

    edges
}

fn aabb_edges(aabb: Aabb) -> Vec<(Vector3, Vector3)> {
    let corners: Vec<Vector3> = (0..8).map(|i| aabb.get_corner(i)).collect();

    let mut edges = Vec::with_capacity(12);

    for a in 0..8 {
        for b in (a + 1)..8 {
            // Two corners form an edge if their indices differ by exactly 1 bit
            let diff = a ^ b;
            if diff == 1 || diff == 2 || diff == 4 {
                edges.push((corners[a as usize], corners[b as usize]));
            }
        }
    }

    edges
}
