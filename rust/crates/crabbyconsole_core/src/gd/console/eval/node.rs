//! Node-specific commands.

use color_eyre::eyre::{ContextCompat as _, eyre};
use crabbyconsole_clap::eval_definition::NodeAction;
use crabbyconsole_misc::{
    async_util::await_untyped_signal, gd::async_node::AsyncGd, util::get_current_scene,
};
use futures::future::select_all;
use godot::{classes::PackedScene, prelude::*};

use crate::gd::console::{CrabConsole, eval::ClapSubAction, util::reload_node};

impl ClapSubAction for NodeAction {
    async fn handle(
        self,
        console: AsyncGd<CrabConsole>,
    ) -> Result<Variant, color_eyre::eyre::Error> {
        match self {
            NodeAction::Spawn { name, path, pos } => {
                // TODO allow 2D spawns as well
                // better idea: pass an expression here so it can be both 2d or 3d, more flexible

                let path = path.join(" ");
                let scene = try_load::<PackedScene>(&path)?; // TODO this gives duplicate error cause, which is ugly

                let mut target = get_current_scene().expect("no current scene");
                // TODO get_current_scene can panic if you call get_tree().unload_current_scene()...
                // Maybe add them to /root/ instead then?

                let mut instantiated =
                    scene.instantiate().context("failed to instantiate scene")?;

                target.add_child(&instantiated);

                if let Some(name) = name {
                    instantiated.set_name(&name);
                }

                let var = Variant::from(instantiated.clone());

                if let Ok(mut node3d) = instantiated.clone().try_cast::<Node3D>()
                    && let Some(pos) = pos
                {
                    let resolved = console.resolve_position_3d(&pos)?;
                    // Note - the ? makes it so if the position resolve fails, it spawns the node, but doesn't position it at all.
                    // May wanna just not spawn the node at all then?

                    node3d.set_global_position(resolved);
                }

                Ok(var)
            }

            NodeAction::Find {
                nodepath_needle,
                all,
                path,
                typ,
            } => {
                let nodepath_needle = nodepath_needle.join(" ");

                // if `nodepath_needle` is empty, it will pass an empty string, which will find all nodes.
                // Because it does path.contains("") which is always true.

                if all {
                    let nodes = console
                        .find_nodes_by_nodepath_needle(&nodepath_needle, typ.as_deref())
                        .into_iter();

                    if path {
                        Ok(Variant::from(
                            nodes
                                .map(|node| node.get_path().to_string())
                                .collect::<Vec<_>>()
                                .join("\n"),
                        ))
                    } else {
                        Ok(Variant::from(nodes.map(Variant::from).collect::<Vec<_>>()))
                    }
                } else {
                    console
                        .find_node_by_nodepath_needle(&nodepath_needle, typ.as_deref())
                        .map(|node| {
                            if path {
                                Variant::from(node.get_path())
                            } else {
                                Variant::from(node)
                            }
                        })
                }
            }

            NodeAction::List => Ok(console.find_nodes_variant("*")),

            NodeAction::Reload {
                nodepath_needle,
                all,
            } => {
                let nodepath_needle = nodepath_needle.join(" ");

                let mut targets =
                    console.find_by_nodepath_needle_infallible(&nodepath_needle, None, all);

                for target in &mut targets {
                    reload_node(target.clone())?;
                }

                Ok(Variant::from(format!(
                    "Reloaded {} nodes:\n{:?}",
                    targets.len(),
                    targets,
                )))
            }

            NodeAction::FromId { instance_id } => Ok(Variant::from(
                // Note: u64 as i64 is lossless, so converting back and forth seems harmless
                Gd::<Node>::try_from_instance_id(InstanceId::from_i64(instance_id.get() as i64))
                    .map_err(|_| {
                        // We cannot return the raw error type from Gd::try_from_instance_id, since its Err variant contains a Variant.
                        // That means it's not thread safe. So instead, we erase the variant from it:
                        // e.into_erased() -> except it gives a vague error message, so do this instead:
                        eyre!(
                            "No Node found with instance id {instance_id} - \
                            maybe it was freed or a non-Node type?"
                        )

                        //Note: e.into_erased() gives bad errors:
                        // 1. if instance id isn't a node -> Error: given object cannot be cast to target type
                        // 2. if instance id doesn't exist at all -> Error: `Gd` cannot be null
                        // And since the enums are private, we cannot detect these cases reliably, unfortunately.
                    })?,
            )),

            NodeAction::Signals { nodepath_needle } => {
                let nodepath_needle = nodepath_needle.join(" ");
                let node = console.find_node_by_nodepath_needle(&nodepath_needle, None)?;

                // TODO allow passing a blacklist of signals NOT to watch.
                // Otherwise e.g. the `script_changed` signal on CrabbyConsole messes up everything.

                // Turn all signals into pinned futures
                let signals = node.get_signal_list();
                let mut futures = vec![];
                for signal in signals.iter_shared() {
                    let signal_name = signal
                        .get("name")
                        .expect("signal has no name") // this should never panic unless Godot changes its API
                        .to::<String>();

                    let node = node.clone(); // fast clone
                    futures.push(Box::pin(async move {
                        (
                            // When the future completes, also return the signal name.
                            // Otherwise we can't distinguish between the different signals coming in.
                            signal_name.clone(),
                            await_untyped_signal(Signal::from_object_signal(&node, &signal_name))
                                .await,
                        )
                    }));
                }

                // Wait for any signal to arrive
                // TODO what happens if object is freed in the meantime?
                // TODO can we make it so if multiple futures are ready, it returns a list of all ready ones?
                // So we can detect 2 or more signals emitting on the exact same frame?
                let ((signal_name, signal_args), _, _) = select_all(futures).await;

                Ok(Variant::from(varray![signal_name, signal_args]))
            }
        }
    }
}
