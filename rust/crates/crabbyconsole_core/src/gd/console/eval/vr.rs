use std::ops::ControlFlow;

use color_eyre::eyre::{OptionExt, Report, bail, ensure};
use crabbyconsole_clap::{clap_util::BoolArg, vr::VrAction};
use crabbyconsole_misc::{
    async_util::loop_over_unhandled_input_events,
    gd::async_node::{AsyncGd, AsyncNode},
    util::get_camera_3d,
};
use godot::{
    classes::{InputEventKey, Sprite3D, SubViewport, ViewportTexture, sprite_base_3d::DrawFlags},
    prelude::*,
};

use crate::gd::console::{CrabConsole, eval::ClapSubAction};

#[derive(Debug, Clone)]
pub struct VrState {
    sprite: Gd<Sprite3D>,
    sprite_parent: Gd<Node>,
    // vr_camera: Gd<Camera3D>, // don't store this, panics if you switch scenes and then call any vr command, use get_camera_3d() instead
    // viewport: Gd<SubViewport>, // seems unneeded?
    follow_camera: bool,
}

pub fn enter_vr(mut console: AsyncGd<CrabConsole>) -> Result<Variant, Report> {
    // If we already are in VR mode, bail out
    ensure!(
        console.bind().vr_state.is_none(),
        "can't enable VR mode - already enabled"
    );

    let vr_camera = get_camera_3d().ok_or_eyre("no Camera3D in the scene, can't enter VR mode")?;

    // Create 3D plane to put the console on
    let mut sprite = Sprite3D::new_alloc();
    let mut viewport = SubViewport::new_alloc();
    viewport.set_size(Vector2i::new(960, 540)); // 720p may be too big
    viewport.set_transparent_background(true);

    // Add it to the root
    let mut sprite_parent = console.get_root();
    sprite_parent.add_child(&viewport);

    let vp_tex: Gd<ViewportTexture> = viewport.get_texture().expect("no viewport texture");

    // Align quad to camera (with scale set to ONE)
    sprite.set_global_transform(Transform3D::new(
        vr_camera.get_global_transform().basis.orthonormalized(),
        vr_camera.get_global_position(),
    ));
    // Put the console at 0.6 meters distance from the camera
    sprite.translate_object_local(Vector3::new(0., 0., -0.6));
    sprite.set_texture(&vp_tex);

    // Check Sprite3D docs to see the default values of all draw flags
    sprite.set_draw_flag(DrawFlags::DISABLE_DEPTH_TEST, true);
    sprite.set_draw_flag(DrawFlags::DOUBLE_SIDED, true); // may be default already but eh

    // Calculate pixel size based on height, so it can become wider without the text becoming smaller
    let world_height = 0.3; // meters 
    let tex_height = vp_tex.get_height();
    sprite.set_pixel_size(world_height / tex_height as f32);

    console.get_root().add_child(&sprite);

    // Now reparent ourselves to be inside the viewport
    console.gd_mut().reparent(&viewport);

    // Focus the textbox here so we can type in VR
    console.bind_mut().nodes.line_edit.grab_focus();

    // TODO maybe do DisplayServer.virtual_keyboard_show() here to show the VR keyboard?

    // Start waiting for keyboard events
    console
        .bind_mut()
        .bound_task()
        .new({
            let mut viewport = viewport.clone(); // fast clone
            async move |_| {
                loop_over_unhandled_input_events(|e| {
                    if let Ok(key) = e.try_cast::<InputEventKey>() {
                        // forward to viewport
                        tracing::debug!(?key);
                        viewport.push_input(&key);
                    }

                    ControlFlow::Continue::<()>(())
                })
                .await;
            }
        })
        .spawn();

    console.bind_mut().vr_state = Some(VrState {
        sprite,
        sprite_parent: sprite_parent.upcast(),
        follow_camera: false,
    });

    // Last, but certainly not least, make the console actually visible:
    console.bind_mut().nodes.canvas_layer.show();

    Ok(Variant::from("VR mode enabled."))
}

impl ClapSubAction for VrAction {
    async fn handle(self, mut console: AsyncGd<CrabConsole>) -> Result<Variant, Report> {
        match self {
            VrAction::Enter => enter_vr(console), // TODO this can be removed now? it does give better errors than CrabbyConsole.enter_vr() though

            VrAction::Scale { scale } => {
                // fast clone
                let Some(VrState {
                    sprite: mut sprite3d,
                    ..
                }) = console.bind().vr_state.clone()
                else {
                    bail!("please enable VR mode first");
                };

                sprite3d.set_scale(Vector3::ONE * scale);
                Ok(Variant::from(sprite3d.get_scale()))
            }

            VrAction::Follow { follow } => {
                let Some(VrState {
                    sprite,
                    sprite_parent,

                    follow_camera: old_value,
                    ..
                }) = &mut console.bind_mut().vr_state
                else {
                    bail!("please enable VR mode first");
                };

                // TODO maybe prioritize OverrideCamera3D here? for debugging?
                let vr_camera = get_camera_3d().ok_or_eyre("no Camera3D in scene")?;

                let new_value = match follow {
                    BoolArg::Set(value) => value,
                    BoolArg::Toggle => !*old_value,
                };

                match (*old_value, new_value) {
                    (true, false) => {
                        // unparent sprite from camera, and put it back to its original parent
                        sprite
                            .reparent_ex(&*sprite_parent)
                            .keep_global_transform(true)
                            .done();
                    }
                    (false, true) => {
                        // parent sprite to camera
                        sprite
                            .reparent_ex(&vr_camera)
                            .keep_global_transform(true)
                            .done();
                    }
                    _ => {
                        // nothing to do, value didn't change
                    }
                }

                // Store the new value inside of vr_state
                *old_value = new_value;

                Ok(Variant::from(if new_value {
                    "Following camera"
                } else {
                    "Unfollowing camera"
                }))
            }
            VrAction::Expand => {
                // Move the vsplitter all the way down
                console.bind_mut().nodes.vsplitter.set_split_offset(999_999); // don't use i32::MAX here, or it overflows

                Ok(Variant::nil())
            }
        }
    }
}
