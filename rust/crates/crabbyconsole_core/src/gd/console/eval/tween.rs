use color_eyre::eyre::{Report, ensure};
use crabbyconsole_clap::tween::TweenAction;
use crabbyconsole_misc::{async_tween::AsyncAnim as _, gd::async_node::AsyncGd};
use futures::future::join_all;
use godot::{classes::Json, prelude::*};

use crate::gd::console::{CrabConsole, eval::ClapSubAction};

impl ClapSubAction for TweenAction {
    async fn handle(self, console: AsyncGd<CrabConsole>) -> Result<Variant, Report> {
        match self {
            TweenAction::Position {
                node,
                from,
                to,
                duration,
                all,
            } => {
                let tween = console.async_anim();
                let to = console.resolve_position_3d(&to)?;
                let from = match from {
                    Some(arg) => Some(console.resolve_position_3d(&arg)?),
                    None => None,
                };

                let node3ds = console
                    .find_by_pattern_infallible(&node, all)
                    .into_iter()
                    .filter_map(|node| node.try_cast::<Node3D>().ok())
                    .collect::<Vec<_>>(); // If !all, will contain only 1 node

                // TODO maybe allow parallel/serial toggle?
                let mut futures = vec![];
                for node3d in &node3ds {
                    let tween = tween.clone();
                    futures.push(async move {
                        tween
                            .move_to(node3d, to, duration as f32)
                            .maybe_from(from)
                            .call()
                            .await;
                    });
                }

                join_all(futures).await; //TODO use FuturesUnordered here maybe? If you get perf problems
                Ok(Variant::from(format!(
                    "Performed {} tweens on the following nodes: {node3ds:?}",
                    node3ds.len()
                )))
            }

            TweenAction::Property {
                node,
                prop,
                from,
                to,
                duration,
                all,
            } => {
                let tween = console.async_anim();

                // DO NOT USE str_to_var here, it's unsafe! See https://github.com/godotengine/godot/issues/80562
                // Instead, use JSON.parse_string.
                // We may need a full-blown GDscript parser here again instead of JSON.parse_string...
                // Otherwise we can't parse Vector3, Color, Transform3Ds, etc
                let safe_parse_variant = |x: &String| Json::parse_string(x);

                let to = safe_parse_variant(&to);
                let from = from.map(|from| safe_parse_variant(&from));
                let nodes = console
                    .find_by_pattern_infallible(&node, all) // If !all, will contain only 1 node
                    .into_iter()
                    .filter(|node| {
                        let original_prop = node.get(&prop);
                        if original_prop.is_nil() {
                            tracing::warn!(
                                "Node {} doesn't have property '{}', skipping tween...",
                                node,
                                prop
                            );
                            return false;
                        }
                        true
                    })
                    .collect::<Vec<_>>();

                ensure!(to.get_type() != VariantType::NIL, "<TO> must not be nil");

                if let Some(from) = &from {
                    ensure!(
                        from.get_type() == to.get_type(),
                        "<FROM> and <TO> have different types: {} vs {}",
                        from.get_type().godot_type_name(),
                        to.get_type().godot_type_name(),
                    );
                }

                let mut futures = vec![];
                for node in &nodes {
                    // What is this, The Clone Wars?
                    let tween = tween.clone();
                    let from = from.clone();
                    let to = to.clone();
                    let prop = prop.clone();

                    let original_prop = node.get(&prop);

                    // This prevents the tween from panicking
                    // TODO - doesn't work with :tween prop -a * global_position Vector3(0,0,0) 10
                    // Maybe just continue in that case
                    ensure!(
                        original_prop.get_type() == to.get_type(),
                        "<TO> has unexpected type: expected {}, got {}",
                        original_prop.get_type().godot_type_name(),
                        to.get_type().godot_type_name(),
                    );

                    futures.push(async move {
                        tween
                            .interpolate_to(&node.clone().upcast(), &prop, to, duration as f32)
                            .maybe_from(from)
                            .call()
                            .await;
                    });
                }

                join_all(futures).await;
                Ok(Variant::from(format!(
                    "Performed {} tweens on the following nodes: {nodes:?}",
                    nodes.len()
                )))
            }
        }
    }
}
