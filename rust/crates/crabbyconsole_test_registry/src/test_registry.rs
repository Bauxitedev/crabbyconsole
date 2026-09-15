use std::panic::{AssertUnwindSafe, catch_unwind};

use godot::global::{godot_error, godot_print};

pub struct GodotTest {
    pub name: &'static str,
    pub func: fn(),
}

// Collect all GodotTests
inventory::collect!(GodotTest);

pub fn get_tests_from_inventory() -> Vec<&'static GodotTest> {
    inventory::iter::<GodotTest>().collect()
}

/// Run a test and return `true` if it passed, else `false`.
pub fn run_one(name: &str) -> bool {
    match get_tests_from_inventory().iter().find(|t| t.name == name) {
        Some(test) => match catch_unwind(AssertUnwindSafe(test.func)) {
            Ok(_) => {
                godot_print!("[PASS] {name}");
                true
            }
            Err(_) => {
                // Note: the panic info seems to get printed automatically, so we don't need to print it here
                godot_print!("[FAIL] {name}");
                false
            }
        },
        None => {
            godot_error!("no test named {name}");
            false
        }
    }
}

pub fn list_names() -> Vec<&'static str> {
    get_tests_from_inventory().iter().map(|t| t.name).collect()
}
