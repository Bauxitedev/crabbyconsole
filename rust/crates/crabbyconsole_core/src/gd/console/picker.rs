use std::ops::ControlFlow;

use crabbyconsole_misc::{
    async_tween::AsyncAnim as _,
    async_util::loop_over_handled_input_events,
    gd::async_node::AsyncGd,
    util::{get_camera_3d, get_root, get_viewport, project_aabb_to_2d, truncate_with_ellipsis},
};
use futures_lite::future::or;
use godot::{
    classes::{
        CollisionObject2D, Control, InputEventMouseButton, InputEventMouseMotion, Node2D,
        PhysicsRayQueryParameters3D, PopupMenu, RenderingServer, Sprite2D,
    },
    global::MouseButton,
    obj::Singleton as _,
    prelude::*,
};
use ordered_float::OrderedFloat;
use scopeguard::defer;
use tap::Tap as _;

use crate::gd::console::{CrabConsole, util::get_node3d_global_aabb};

impl CrabConsole {
    #[tracing::instrument(skip_all)]
    pub(super) async fn picker_task(mut self: AsyncGd<Self>) {
        tracing::debug!("picker task starting...");

        let mut picker = self.bind().nodes.picker.clone(); // fast clone
        let skip_array = self
            .gd()
            .get_tree()
            .get_nodes_in_group("CRABBYCONSOLE_PICKER_SKIP"); // <-- do this before remove_child, otherwise it will pick the picker itself
        let mut picker_parent = picker.get_parent().expect("picker has no parent");
        picker_parent.remove_child(&picker); // <-- we will re-add the picker later

        let vsplitter = self.bind().nodes.vsplitter.clone(); // fast clone

        let popup = self
            .bind()
            .nodes
            .ellipsis_button
            .get_popup()
            .unwrap()
            .clone();

        // Free the picker node when the picker task ends (it may not be in the tree, so will NOT be automatically freed)
        let mut picker2 = picker.clone();
        defer! {
            if picker2.is_instance_valid() {
                tracing::debug!("freeing picker...");
                picker2.queue_free();
            }
        }

        // skip these nodes, so we don't pick them
        let skip = Vec::from(&skip_array);
        tracing::debug!(?skip);

        while let Ok((id,)) = popup.signals().id_pressed().to_fallible_future().await {
            // Hide the autocomplete label, so it doesn't block the screen
            self.bind_mut().nodes.autocomplete_label.hide();

            let pick_type = match id {
                0 => PickType::Control,
                1 => PickType::Node2D,
                2 => PickType::Node3D,
                id => {
                    tracing::warn!("unknown popup menu id {id}");
                    continue;
                }
            };

            // Slide up the console
            // vsplitter.set_split_offset(i32::MIN); // <-- uncomment to disable the animation

            // the target value is -395 on 1080p and -614 on 1440p, but will be lower on higher resolutions
            // additionally, it may be entirely off depending on dpi and Godot stretch mode...
            let target_split_offset = -500;

            // Only perform the anim if the split offset is too big, so we only move the console out of the way if it's actually in the way
            if vsplitter.get_split_offset() >= target_split_offset {
                self.async_anim()
                    .interpolate_to(
                        &vsplitter.clone().upcast(),
                        "split_offset",
                        target_split_offset,
                        0.4,
                    )
                    .call()
                    .await;
            }

            // Note this gets the game's root viewport, not the console's viewport.
            let global_vp = self.gd().get_viewport().unwrap();
            picker.set_global_position(global_vp.get_mouse_position());
            picker_parent.add_child(&picker);

            let mut state = LoopState {
                hovered: None,
                hovered_ones: vec![],
                selected_slot: 0, // None?
                picker: picker.clone(),
            };

            let poll_picker = async {
                loop {
                    // Wait for either 1. mouse movement or 2. scroll
                    let mouse_moved = loop_over_handled_input_events(|e| match e
                        .try_cast::<InputEventMouseMotion>(
                    ) {
                        Ok(e) => ControlFlow::Break(e.get_global_position()),
                        _ => ControlFlow::Continue(()),
                    });

                    let scrolled = loop_over_handled_input_events(|e| {
                        match e.try_cast::<InputEventMouseButton>() {
                            Ok(e) if e.is_pressed() => match e.get_button_index() {
                                MouseButton::WHEEL_UP => ControlFlow::Break(1), // e.get_factor() for fractional scrolling
                                MouseButton::WHEEL_DOWN => ControlFlow::Break(-1),
                                _ => ControlFlow::Continue(()),
                            },
                            _ => ControlFlow::Continue(()),
                        }
                    });

                    // TODO move tokio::select! into loop_over_handled_input_events? so we can combine the two futures?
                    // Keep code here to a minimum, it will not be formatted due to being inside of a macro
                    tokio::select! {
                        mouse_pos = mouse_moved => {
                            state.handle_mouse_pos(mouse_pos, &pick_type, &skip);
                        }
                        scroll_amount = scrolled => {
                            state.handle_scroll(scroll_amount);
                        }
                    }
                }
            };

            // We use wait_for_handled_input_event instead of wait_for_unhandled_input_event.
            // Otherwise this will not trigger if you e.g. click on a button that consumes the event
            let pick_completed =
                loop_over_handled_input_events(|e| match e.try_cast::<InputEventMouseButton>() {
                    Ok(e) if e.get_button_index() == MouseButton::LEFT && e.is_pressed() => {
                        ControlFlow::Break(true)
                    }
                    Ok(e) if e.get_button_index() == MouseButton::RIGHT && e.is_pressed() => {
                        ControlFlow::Break(false)
                    }
                    _ => ControlFlow::Continue(()), // any other button is ignored
                                                    // TODO maybe also check if user pressed "Esc"?
                });

            // poll_picker never returns, so this always returns the value of the right-hand future
            let left_clicked = or(poll_picker, pick_completed).await;

            picker_parent.remove_child(&picker);

            if left_clicked && let Some(hovered) = state.hovered {
                // fast clone
                self.clone()
                    .show_nodepath_popup_and_insert_text(hovered)
                    .await;
            } // if not left clicked, or nothing was hovered, do nothing
        }

        tracing::warn!("picker task finished");
    }

    async fn show_nodepath_popup_and_insert_text(mut self: AsyncGd<Self>, hovered: HoveredNode) {
        // This allow user to pick format A, B, C or D in a dropdown
        // A = get_node("/root/foo")
        // B = root/foo
        // C = from_instance_id([instance id])
        // D = :node from-id [instance id]
        // B is useful for e.g. :watch signals :node find B
        // C is useful if the node path is extremely long
        // D is useful to avoid using GDScript

        let mut menu = PopupMenu::new_alloc();
        self.gd_mut().add_child(&menu);

        // We allow the user to pick 4 different kind of formats:
        let text_a = format!(r#"get_node("{}")"#, hovered.get_path());
        let text_b = hovered.get_path().to_string();
        let text_c = format!(r#"instance_from_id({})"#, hovered.get_inner().instance_id());
        let text_d = format!(r#":node from-id {}"#, hovered.get_inner().instance_id());

        // Truncate the strings to avoid creating a massive menu for deep NodePaths
        menu.add_separator_ex().label("Which format to use?").done();
        menu.add_item_ex(&truncate_with_ellipsis(&text_a, 100))
            .id(0)
            .done();
        menu.add_item_ex(&truncate_with_ellipsis(&text_b, 100))
            .id(1)
            .done();
        menu.add_item_ex(&truncate_with_ellipsis(&text_c, 100))
            .id(2)
            .done();
        menu.add_item_ex(&truncate_with_ellipsis(&text_d, 100))
            .id(3)
            .done();
        // TODO use smaller monospace font maybe?

        // create the future before showing the menu, to ensure we connect in time to the signal
        let menu_selected_fut = menu.signals().id_pressed().to_fallible_future();
        let menu_hidden_fut = menu.signals().popup_hide().to_fallible_future(); //  `close_requested` does not fire at all
        let mouse_pos = get_viewport().get_mouse_position();
        menu.popup_ex() // using `rect` will automatically limit the menu to the screen bounds
            .rect(Rect2i::new(
                Vector2i::new(mouse_pos.x.round() as i32, mouse_pos.y.round() as i32),
                Vector2i::new(100, 100),
            ))
            .done();

        tracing::debug!("waiting for user to pick...");

        let selected = tokio::select! {
            biased; // <-- important: ensures `menu_selected` has priority over `menu_hidden`
            Ok((selected,)) = menu_selected_fut => selected,
            Ok(()) = menu_hidden_fut => {
                // menu was hidden while waiting for it, so cancel
                return;
            },
            else => {
                // menu was freed while waiting for it, so cancel
                return;
            },
        };

        tracing::debug!("user picked {selected}");

        let text_to_insert = match selected {
            0 => text_a,
            1 => text_b,
            2 => text_c,
            3 => text_d,
            _ => return, // invalid id, so cancel
        };

        menu.queue_free(); // important, or it will leak, since we create a new one every time

        let mut line_edit = self.bind().nodes.line_edit.clone(); //fast clone

        // TODO insert text at the caret, not just at the end
        let new_text = line_edit.get_text().to_string() + &text_to_insert;
        line_edit.set_text(&new_text);
    }
}

// Little helper struct so we can use methods instead of closures with tokio::select!.
// Otherwise we get double-borrows (since every closure holds a mutable borrow to all captured vars).
struct LoopState {
    hovered: Option<HoveredNode>,
    hovered_ones: Vec<HoveredNode>, // so we can do HoveredNode.get_index() and HoveredNode.get_under_mouse and HoveredNode.get_global_bounding_rect
    selected_slot: usize,
    picker: Gd<Node2D>,
}

impl LoopState {
    fn handle_mouse_pos(&mut self, mouse_pos: Vector2, pick_type: &PickType, skip: &[Gd<Node>]) {
        self.picker.set_global_position(mouse_pos); // or global_vp.get_mouse_position();

        // hovered = global_vp.gui_get_hovered_control(); // <- unusable: always returns the console subviewport, since it's above everything...
        self.hovered_ones =
            HoveredNode::get_under_mouse(pick_type, &get_root().upcast(), mouse_pos, skip);

        // Get the one by highest z-index
        // TODO: we need to take "is_above_parent" and "z_as_relative" into account.
        // Doing this accurately basically means re-implementing godot's entire ordering system, so extremely complex
        // ...let's just do the bare minimum

        // sort by z index
        self.hovered_ones
            .sort_by_key(|c| OrderedFloat(c.get_z_index()));

        self.selected_slot = self.hovered_ones.len().saturating_sub(1);
        self.hovered = self.hovered_ones.get(self.selected_slot).cloned(); // can be None if empty

        if let Some(hovered) = &self.hovered {
            let slot_text = format!(
                "[{}/{}]\n(scroll to change slot)",
                self.selected_slot + 1,
                self.hovered_ones.len()
            );
            // see console_picker.gd - wait why are we going through the script again?
            // this code seems fragile, maybe just access them directly via %?
            self.picker.set("text", &Variant::from(hovered.get_path()));
            self.picker.set("slot_text", &Variant::from(slot_text));
            self.picker.set("panel_visible", &Variant::from(true));

            self.picker.call(
                "set_panel_rect_global",
                &[Variant::from(hovered.get_global_bounding_rect())],
            );
        } else {
            self.picker.set(
                "text",
                &Variant::from(format!("<no {pick_type:?} under cursor>")),
            );
            self.picker.set("slot_text", &Variant::from(""));
            self.picker.set("panel_visible", &Variant::from(false));
        }
    }

    fn handle_scroll(&mut self, scroll_amount: i64) {
        // handle scroll

        let hovered_ones_len = self.hovered_ones.len();
        if hovered_ones_len == 0 {
            // do nothing, since no nodes are under the cursor atm
            return;
        }

        self.selected_slot = (self.selected_slot as i64 - scroll_amount)
            .rem_euclid(hovered_ones_len as i64) as usize;
        let newly_hovered = self.hovered_ones[self.selected_slot].clone(); // this should never panic
        self.hovered = Some(newly_hovered.clone());

        let slot_text = format!(
            "[{}/{}]\n(scroll to change slot)",
            self.selected_slot + 1,
            hovered_ones_len
        );
        self.picker
            .set("text", &Variant::from(newly_hovered.get_path()));

        self.picker.set("slot_text", &Variant::from(slot_text));

        self.picker.call(
            "set_panel_rect_global",
            &[Variant::from(newly_hovered.get_global_bounding_rect())],
        );
    }
}

#[derive(Debug, Clone)]
enum PickType {
    Control,
    Node2D,
    Node3D,
}

#[derive(Clone)]
enum HoveredNode {
    Control(Gd<Control>),
    Node2D(Gd<Node2D>),
    Node3D(Gd<Node3D>, Vector3), // raycast hit pos
}

impl HoveredNode {
    fn get_z_index(&self) -> f32 {
        match self {
            HoveredNode::Control(c) => c.get_z_index() as f32,
            HoveredNode::Node2D(n2d) => n2d.get_z_index() as f32,
            HoveredNode::Node3D(_, hitpos) => {
                // If the user tries to pick a 3D node when no Camera3D exists, we can't return anything meaningful here.
                // So just return a dummy value.
                let Some(cam) = get_camera_3d() else {
                    return 0.0;
                };

                // Negate it, since higher z index = before the others
                -hitpos.distance_squared_to(cam.get_global_position())
            }
        }
    }

    fn get_under_mouse(
        pick_type: &PickType,
        root: &Gd<Node>,
        mouse_pos: Vector2,
        skip: &[Gd<Node>],
    ) -> Vec<Self> {
        // TODO if you have a lot of skips maybe use a HashSet instead for performance
        match pick_type {
            PickType::Control => get_controls_under_mouse(root, mouse_pos, skip)
                .into_iter()
                .map(HoveredNode::Control)
                .collect(),
            PickType::Node2D => get_node2ds_under_mouse(root, mouse_pos, skip)
                .into_iter()
                .map(HoveredNode::Node2D)
                .collect(),
            PickType::Node3D => get_node3ds_under_mouse(mouse_pos, skip)
                .into_iter()
                .map(|(n3d, hitpos)| HoveredNode::Node3D(n3d, hitpos))
                .collect(),
        }
    }

    fn get_global_bounding_rect(&self) -> Rect2 {
        match self {
            HoveredNode::Control(c) => get_control_global_bounding_rect(c),
            HoveredNode::Node2D(n2d) => get_node2d_global_bounding_rect(n2d),
            HoveredNode::Node3D(n3d, _) => get_node3d_global_bounding_rect(n3d),
        }
    }

    fn get_path(&self) -> NodePath {
        // TODO why not self.get_inner().get_path()?
        match self {
            HoveredNode::Control(node) => node.get_path(),
            HoveredNode::Node2D(node) => node.get_path(),
            HoveredNode::Node3D(node, _) => node.get_path(),
        }
    }

    fn get_inner(&self) -> Gd<Node> {
        match self {
            HoveredNode::Control(gd) => gd.clone().upcast(),
            HoveredNode::Node2D(gd) => gd.clone().upcast(),
            HoveredNode::Node3D(gd, _) => gd.clone().upcast(),
        }
    }
}

/// Recursively finds ALL visible Controls under `mouse_pos`, skipping specific nodes.
/// Order = tree traversal order (parents before children, siblings in child order), so NOT z-sorted.
/// You can do that yourself later manually
fn get_controls_under_mouse(
    root: &Gd<Node>,
    mouse_pos: Vector2,
    skip: &[Gd<Node>],
) -> Vec<Gd<Control>> {
    fn collect_controls_under_mouse(
        root: &Gd<Node>,
        mouse_pos: Vector2,
        skip: &[Gd<Node>],
        out: &mut Vec<Gd<Control>>,
    ) {
        for child in root.get_children().iter_shared() {
            if let Ok(control) = child.clone().try_cast::<Control>()
                && !skip.iter().any(|skip| *skip == control.clone().upcast())
                && control.is_visible()
                && get_control_global_bounding_rect(&control).contains_point(mouse_pos)
            {
                out.push(control);
            }

            if child.get_child_count() > 0 {
                collect_controls_under_mouse(&child, mouse_pos, skip, out);
            }
        }
    }

    let mut results: Vec<Gd<Control>> = Vec::new();
    collect_controls_under_mouse(root, mouse_pos, skip, &mut results);
    results
}

pub fn get_control_global_bounding_rect(control: &Gd<Control>) -> Rect2 {
    // Control.get_global_rect takes the following into account:
    // - Control.scale
    // - Control.pivot_offset
    // It does NOT take into account:
    // - Control.rotation
    // - Camera2D position/rotation/zoom
    // So, let's make our own

    // This seems to take pivot_offset into account automagically
    let xform = control.get_global_transform_with_canvas(); // "_with_canvas" = take Cam2d into account
    let size = control.get_size();

    let corners = [
        xform * Vector2::new(0.0, 0.0),
        xform * Vector2::new(size.x, 0.0),
        xform * Vector2::new(0.0, size.y),
        xform * Vector2::new(size.x, size.y),
    ];

    // Create a Rect2 that encompasses all corner points, even if they're rotated.
    corners
        .iter()
        .map(|c| Rect2::new(*c, Vector2::ZERO))
        .reduce(|l, r| l.merge(r))
        .unwrap() // safe unwrap - corners cannot be empty
}

fn get_node2ds_under_mouse(
    root: &Gd<Node>,
    mouse_pos: Vector2,
    skip: &[Gd<Node>],
) -> Vec<Gd<Node2D>> {
    fn collect_node2ds_under_mouse(
        root: &Gd<Node>,
        mouse_pos: Vector2,
        skip: &[Gd<Node>],
        out: &mut Vec<Gd<Node2D>>,
    ) {
        for child in root.get_children().iter_shared() {
            if let Ok(node2d) = child.clone().try_cast::<Node2D>()
                && !skip.iter().any(|skip| *skip == node2d.clone().upcast())
                && node2d.is_visible()
                && get_node2d_global_bounding_rect(&node2d).contains_point(mouse_pos)
            {
                out.push(node2d);
            }

            if child.get_child_count() > 0 {
                collect_node2ds_under_mouse(&child, mouse_pos, skip, out);
            }
        }
    }

    let mut results: Vec<Gd<Node2D>> = Vec::new();
    collect_node2ds_under_mouse(root, mouse_pos, skip, &mut results);
    results
}

/// Returns bounding rect of a `Node2D` (in global space).
/// (Empty/non-visual nodes will return zero)
///
/// TODO: will not work in exported game if node isn't a `Sprite2D` or a `CollisionObject2D`, due to `debug_canvas_item_get_rect`!
fn get_node2d_global_bounding_rect(node: &Gd<Node2D>) -> Rect2 {
    let mut rs = RenderingServer::singleton();

    let mut local_rect = if let Ok(spr) = node.clone().try_cast::<Sprite2D>() {
        // Note - this does NOT work for AnimatedSprite2D, since it inherits Node2D, NOT Sprite2D.
        // May wanna add custom logic for AnimatedSprite2D, Polygon2D
        // TODO - also add CollisionObject2D - will need to branch manually on all shape types though + deal with shape owner transform, kinda tedious
        spr.get_rect()
    } else if let Ok(mut body) = node.clone().try_cast::<CollisionObject2D>() {
        get_body_local_rect2(&mut body)
    } else {
        // Warning: This function is intended for debugging in the editor, and will return a zero Rect2 in exported projects.
        rs.debug_canvas_item_get_rect(node.get_canvas_item())
            .tap(|r| tracing::debug!(?r, ?node, "debug_canvas_item_get_rect"))
    };

    // If rect is zero-sized, grow it a little, else there's no way we can hover over it.
    let eps = 0.01;
    if local_rect.size.x.abs() < eps && local_rect.size.y.abs() < eps {
        local_rect = local_rect.grow(5.0);
    }

    node.get_global_transform_with_canvas() * local_rect
}
fn get_body_local_rect2(body: &mut Gd<CollisionObject2D>) -> Rect2 {
    let mut rects: Vec<Rect2> = Vec::new();

    for owner_id in body.get_shape_owners().as_slice().iter().copied() {
        let owner_id = owner_id as u32;
        let shape_count = body.shape_owner_get_shape_count(owner_id);

        for shape_id in 0..shape_count {
            let Some(shape) = body.shape_owner_get_shape(owner_id, shape_id) else {
                continue;
            };

            let xform = body.shape_owner_get_transform(owner_id);
            rects.push(xform * shape.get_rect());
        }
    }

    rects
        .into_iter()
        .reduce(|l, r| l.merge(r))
        .unwrap_or_default()
}

fn get_node3ds_under_mouse(mouse_pos: Vector2, skip: &[Gd<Node>]) -> Vec<(Gd<Node3D>, Vector3)> {
    // TODO this uses a potentially different viewport/camera3d compared to get_node3d_global_bounding_rect().
    // Watch out for discrepancies in split-screen games, or games that use multiple viewports in general!
    let viewport = get_viewport();
    let Some(camera) = viewport.get_camera_3d() else {
        tracing::warn!("no Camera3D in scene, so can't pick any 3D nodes");
        return Vec::new();
    };

    let ray_origin = camera.project_ray_origin(mouse_pos);
    let ray_end = ray_origin + camera.project_ray_normal(mouse_pos) * 1000.0;

    let world = viewport.get_world_3d();
    let Some(world) = world else {
        tracing::warn!("no World");
        return Vec::new();
    };

    let Some(mut space_state) = world.get_direct_space_state() else {
        tracing::warn!("no PhysicsDirectSpaceState3D");
        return Vec::new();
    };

    let mut exclude: Array<Rid> = Array::new();
    let mut nodes = Vec::new();

    loop {
        let mut query = PhysicsRayQueryParameters3D::create_ex(ray_origin, ray_end)
            .exclude(&exclude)
            .done()
            .expect("failed to create PhysicsRayQueryParameters3D"); // <-- not sure if this can ever fail?

        query.set_collide_with_areas(true);
        query.set_collide_with_bodies(true);

        let result = space_state.intersect_ray(&query);

        if result.is_empty() {
            break; // No hit
        }

        // At this point none of the unwraps should fail, unless Godot's API changes in the future:

        let hitpos = result
            .get("position")
            .expect("no position in raycast dict")
            .try_to::<Vector3>()
            .expect("position in raycast dict wasn't a Vector3");
        let collider = result.get("collider").expect("no collider in raycast dict");

        if let Ok(node3d) = collider.try_to::<Gd<Node3D>>()
            && !skip.iter().any(|skip| *skip == node3d.clone().upcast())
        {
            // add node only if it's a node3d and not in skip list
            nodes.push((node3d, hitpos));
        }

        let rid = result.get("rid").expect("no rid in raycast dict");
        if let Ok(rid) = rid.try_to::<Rid>() {
            exclude.push(rid);
        }
    }

    nodes
}

// Note - the picker raycast can only hit physical objects, so no MeshInstance3D, Sprite3D, etc

pub fn get_node3d_global_bounding_rect(n3d: &Gd<Node3D>) -> Rect2 {
    let aabb = get_node3d_global_aabb(n3d);
    let Some(cam) = get_camera_3d() else {
        // If the user tries to pick a 3D node when no Camera3D exists, we can't return anything meaningful here.
        // So just return a dummy rect.
        return Rect2::new(Vector2::ZERO, Vector2::new(10.0, 10.0));
    };
    project_aabb_to_2d(aabb, &cam)
}
