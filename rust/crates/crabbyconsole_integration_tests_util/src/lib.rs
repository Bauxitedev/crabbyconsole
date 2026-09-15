use std::{
    env,
    io::{self, ErrorKind},
    process::Command,
};

const GODOT4_BIN: &str = "GODOT4_BIN";

#[must_use]
pub fn godot_binary() -> String {
    // The `GODOT4_BIN` thing is consistent with godot-rust
    env::var(GODOT4_BIN).unwrap_or_else(|_| "godot".to_string())
}

pub fn godot_version() -> io::Result<String> {
    let bin = godot_binary();

    let output = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(improve_godot_not_found_error)?;

    if !output.status.success() {
        return Err(io::Error::other(format!(
            "godot --version exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn improve_godot_not_found_error(e: io::Error) -> io::Error {
    let bin = godot_binary();
    match e.kind() {
        ErrorKind::NotFound => io::Error::new(
            io::ErrorKind::NotFound,
            if env::var(GODOT4_BIN).is_ok() {
                format!(
                    "env var `{GODOT4_BIN}` was detected, but it points either to a non-existent file (`{bin}`), or we don't have permission to run it ({e})"
                )
            } else {
                format!(
                    "could not find `godot`: please ensure `godot` is in your PATH, or define an env var `{GODOT4_BIN}` to specify the full path of the Godot binary ({e})"
                )
            },
        ),
        _ => e,
    }
}
