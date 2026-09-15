use std::{env, path::PathBuf};

fn main() {
    // Update -  I found a way to trigger a stale build id.
    // Try changing a single line in the crate `crabbyconsole_test_registry`.
    // It rebuilds, but the build id doesn't change.
    // Same goes for `crabbyconsole_core`.
    // So, it ONLY updates when you change `crabbyconsole` itself.

    // So maybe do this anyway? Be careful it doesn't destroy your compile times though.
    // println!("cargo:rerun-if-changed=totally-nonexistent-file");

    emit_actual_profile();
    emit_build_id();
    emit_build_features();
}

fn emit_actual_profile() {
    // Very dumb hack which is needed because cargo does NOT provide a way to get the actual profile name, just "release" or "debug".
    // I want the ACTUAL name, like "release-with-debug"
    // see https://github.com/rust-lang/cargo/issues/2084

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR env var not set"));

    // OUT_DIR looks like:
    // target/<profile>/build/<pkg>-<hash>/out
    // walk up until we find the "build" component, then take its parent's file name
    let profile = out_dir
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "build"))
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    println!("cargo:rustc-env=ACTUAL_PROFILE={profile}");
}

fn emit_build_id() {
    let build_id: u32 = rand::random();
    println!("cargo:rustc-env=BUILD_ID={build_id:08X}");
}

fn emit_build_features() {
    // Checks the activated Cargo features and emits them to an env var `BUILT_FEATURES` at build time.
    let mut features = vec![];
    if env::var("CARGO_FEATURE_TRACY").is_ok() {
        features.push("tracy");
    }
    println!("cargo:rustc-env=BUILD_FEATURES={}", features.join(","));
}
