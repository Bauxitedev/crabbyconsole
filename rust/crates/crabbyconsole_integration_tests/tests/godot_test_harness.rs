use std::{
    env,
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
};

use crabbyconsole_integration_tests_util::{
    godot_binary, godot_version, improve_godot_not_found_error,
};
// Note - do NOT let this file depend directly on the integration test crate
// Because then the console will be compiled twice (release + debug)
use libtest_mimic::{Arguments, Trial};
use rand::RngExt as _;

const INTEGRATION_TEST_SCRIPT: &str = "addons/crabbyconsole/scripts/test/integration_test.gd";

// TODO: cargo nextest seems to spam this before every test:
// > warning: could not find build script output file at .../rust/target/debug/build/..._integration_tests/.../output
// The tests themselves work fine though?

fn main() -> ExitCode {
    // Always use eprintln! in a custom test harness or it messes with nextest
    // This is kind of like sqlx, we "connect" to Godot (instead of postgres) to get the test list.

    let start = std::time::Instant::now();

    let names_path = {
        // TODO - instead of depending on rand here, we can maybe just get the current date in ms and hash() it
        let token: u64 = rand::rng().random();
        let names_path = std::env::temp_dir().join(format!("crabbyconsole_test_names_{token}.txt"));
        eprintln!("generated filename {names_path:?}");
        names_path
    };

    let godot_project_dir = get_godot_project_dir();

    eprintln!("starting godot ({})...", godot_binary());
    let status = Command::new(godot_binary())
        .args([
            "--headless",
            "-s",
            INTEGRATION_TEST_SCRIPT,
            "--",
            "--crabbyconsole-write-tests-to",
            names_path.to_str().unwrap(),
        ])
        .current_dir(godot_project_dir)
        // TODO use output() to capture stdout and stderr automatically
        .stdout(Stdio::null()) // TODO use piped here, NOT inherit, or it breaks nextest.
        .status() // NOTE - we need to NOT make godot inherit our stdout, or it will break nextest.
        // Instead, redirect godot's stdout to our stderr, instead of our stdout.
        .map_err(improve_godot_not_found_error)
        .expect("failed to launch godot binary");

    assert!(
        status.success(),
        "godot --crabbyconsole-test-list exit code non-zero"
    );

    let names_content = std::fs::read_to_string(&names_path).unwrap_or_else(|_| {
        panic!(
            "{names_path:?} was not written by godot --crabbyconsole-test-list, even though godot exited successfully"
        )
    });

    let names: Vec<String> = names_content
        .lines()
        .filter(|l| !l.is_empty())
        .map(|s| s.to_owned())
        .collect();
    assert!(!names.is_empty(), "no #[godot_test] functions found");

    eprintln!(
        "took {:?} to do everything in the test harness to get the test list",
        start.elapsed() // ~1.3-2.4 seconds, seems fast enough
    );

    // Delete the file, to avoid using it twice and causing staleness
    eprintln!("deleting `{names_path:?}`...");
    std::fs::remove_file(&names_path).ok();

    ////////////////////////////////////////////////////////////////

    eprintln!("custom test harness starting");

    let args = Arguments::from_args();

    let mut tests = vec![];

    let test_kind = godot_version().expect("failed to get godot version");
    for name in names {
        // Use .with_ignored_flag(true) to ignore flags
        tests.push(
            Trial::test(name.clone(), move || {
                run_godot_test(&name); // panics if it fails

                Ok(())
            })
            .with_kind(test_kind.clone()),
        );
    }

    libtest_mimic::run(&args, tests).exit_code()
}

fn run_godot_test(test_name: &str) {
    eprintln!("running godot test `{test_name}`...");

    let godot_project_dir = get_godot_project_dir();

    // run godot in the `godot` dir
    let output = Command::new(godot_binary())
        .args([
            "--headless",
            "-s",
            INTEGRATION_TEST_SCRIPT,
            "--",
            "--crabbyconsole-test",
            test_name,
        ])
        .current_dir(godot_project_dir)
        .output()
        .map_err(improve_godot_not_found_error)
        .expect("failed to launch godot binary");
    let status = output.status;

    eprintln!("\n--- GODOT STDOUT ---\n");
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    eprintln!("\n--- GODOT STDERR ---\n");
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    eprintln!("\n--- GODOT DONE ---\n");

    assert!(
        status.success(),
        "godot test '{test_name}' failed (exit code {:?})",
        status.code()
    );

    eprintln!("ran godot test `{test_name}` successfully.");
}

fn get_godot_project_dir() -> PathBuf {
    // Print debug stuff to stderr, or it won't show up in cargo nextest
    eprintln!(
        "PATH = {:?}",
        std::env::var("PATH").expect("no PATH env var"),
    );
    eprintln!(
        "current_dir = {:?}",
        std::env::current_dir().expect("failed to get current dir")
    ); // it's rust/crates/crabbyconsole_integration_tests
    eprintln!("godot_binary = {:?}", godot_binary());

    let rust_workspace_dir =
        env::var("CARGO_WORKSPACE_DIR").expect("no env var CARGO_WORKSPACE_DIR"); // this is /rust
    // go one level up
    let godot_project_dir = PathBuf::from(&rust_workspace_dir)
        .parent()
        .expect("failed to get parent folder of CARGO_WORKSPACE_DIR")
        .join("godot/")
        .canonicalize()
        .expect("failed to canonicalize folder");
    eprintln!("rust_workspace_dir = {rust_workspace_dir:?}");
    eprintln!("godot_project_dir = {godot_project_dir:?}");

    godot_project_dir
}
