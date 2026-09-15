use godot::{
    classes::{InputEvent, node::ProcessMode},
    prelude::*,
};

/// Little helper `AutoLoad` that sends both handled and unhandled input events as a signal.
#[derive(GodotClass, Debug)]
#[class(base=Node)]
pub struct InputEventManager {
    base: Base<Node>,
}

#[godot_api]
impl INode for InputEventManager {
    fn init(base: Base<Node>) -> Self {
        // Note - this also runs in the editor
        Self { base }
    }

    fn ready(&mut self) {
        // Important: this makes sure the signals are emitted even when the game is paused.
        self.base_mut().set_process_mode(ProcessMode::ALWAYS);
    }

    fn input(&mut self, event: Gd<InputEvent>) {
        self.signals().handled_input_event().emit(&event);
    }

    fn unhandled_input(&mut self, event: Gd<InputEvent>) {
        self.signals().unhandled_input_event().emit(&event);
    }
}

#[godot_api]
impl InputEventManager {
    #[signal]
    pub fn unhandled_input_event(event: Gd<InputEvent>);

    #[signal]
    pub fn handled_input_event(event: Gd<InputEvent>);
}
