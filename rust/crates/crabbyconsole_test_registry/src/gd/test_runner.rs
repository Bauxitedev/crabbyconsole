use std::{
    fs::OpenOptions,
    io::{Result, Write},
};

use godot::prelude::*;

use crate::test_registry;

/// This is the Godot class responsible for running CrabbyConsole's integration tests.
///
/// It will be compiled into the final build of the game, so we can also do integration testing in exported games.
/// This does increase compile times + binary size though.
#[derive(GodotClass)]
#[class(base = RefCounted)]
pub struct CrabConsoleTestRunner {
    base: Base<RefCounted>,
}

#[godot_api]
impl IRefCounted for CrabConsoleTestRunner {
    fn init(base: Base<RefCounted>) -> Self {
        Self { base }
    }
}

fn find_cli_arg(arg: &str, args: &PackedArray<GString>) -> Option<String> {
    let mut res: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        if args[i] == arg && i + 1 < args.len() {
            res = Some(args[i + 1].to_string());
            break;
        }
        i += 1;
    }

    res
}

/// Writes a Vec<String> to a new file, rejecting existing files
fn write_lines(filename: &str, lines: Vec<String>) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(filename)?;
    for line in lines {
        writeln!(file, "{}", line)?;
    }
    Ok(())
}

#[godot_api]
impl CrabConsoleTestRunner {
    /// Call this static method from integration_test.gd.
    ///
    /// Returns exit code (0 = test success, 1 = test failure)
    #[func]
    fn run_from_args(args: PackedStringArray) -> i32 {
        // Find the --crabbyconsole-write-tests-to <FILE_NAME> cli arg
        let test_file_name = find_cli_arg("--crabbyconsole-write-tests-to", &args);

        if let Some(test_file_name) = test_file_name {
            // Write the names of all tests to `test_file_name`
            let result = write_lines(
                &test_file_name,
                test_registry::list_names()
                    .iter()
                    .map(|s| (*s).to_owned()) // &'static str -> String
                    .collect(),
            );

            match result {
                Ok(()) => {
                    println!(
                        "Wrote {} tests to `{test_file_name}`",
                        test_registry::list_names().len()
                    );
                    return 0;
                }
                Err(e) => {
                    godot_error!("Failed to write tests to `{test_file_name}`: {e}");
                    return 1;
                }
            }
        }

        // Find the --crabbyconsole-test <TEST_NAME> cli arg
        let test_name: Option<String> = find_cli_arg("--crabbyconsole-test", &args);

        let Some(test_name) = test_name else {
            godot_error!(
                "No --crabbyconsole-test name provided (usage: -- --crabbyconsole-test <name>)"
            );
            return 1;
        };

        // Now run the test
        let passed = test_registry::run_one(&test_name);
        if passed { 0 } else { 1 }
    }
}
