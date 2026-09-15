use godot::{
    classes::{InputEventKey, InputMap},
    global::Key,
    prelude::*,
};

use crate::gd::console::CrabConsole;

struct InputActionDef {
    name: &'static str,
    physical_keys: &'static [Key],
    control: bool, // should Ctrl (or Command on Mac) be pressed?
}

impl CrabConsole {
    /// Adds the default CrabConsole input maps (skipping them if the user redefined them manually)
    #[tracing::instrument(skip_all)]
    pub(super) fn setup_input_maps() {
        let actions: &[InputActionDef] = &[
            InputActionDef {
                name: "crabbyconsole_toggle",
                physical_keys: &[Key::QUOTELEFT], // `
                control: false,
            },
            InputActionDef {
                name: "crabbyconsole_search_toggle",
                physical_keys: &[Key::R],
                control: true,
            },
            InputActionDef {
                name: "crabbyconsole_interrupt",
                physical_keys: &[Key::C],
                control: true,
            },
        ];

        let mut input_map = InputMap::singleton();

        for action in actions {
            let action_name = StringName::from(action.name);

            if input_map.has_action(&action_name) {
                tracing::info!(action = action.name, "input map already added, skipping");
                continue;
            }

            input_map.add_action(&action_name);

            for &key in action.physical_keys {
                let mut event = InputEventKey::new_gd();
                event.set_physical_keycode(key);
                event.set_command_or_control_autoremap(action.control);
                input_map.action_add_event(&action_name, &event);
            }

            tracing::info!(
                action = action.name,
                keys = ?action.physical_keys,
                control = action.control,
                "added input map"
            );
        }
    }
}
