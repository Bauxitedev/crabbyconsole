---
icon: lucide/rocket
---

# CrabbyConsole

CrabbyConsole is a feature-packed dev console for your Godot game, allowing you to run arbitrary GDScript and custom commands during the game. It can be used for debugging, inspecting and modifying your game at runtime, and also for diagnosing performance problems.

Features:

- Custom command system built on the Rust [`clap`](https://crates.io/crates/clap) crate
- Watch variables, expressions and signals
- Performance metrics (frame rate, memory usage, VRAM usage, disk usage, etc)
- Debug drawing, ranging from lines to rectangles, cubes, spheres and text (2D and 3D)
- Plotting system, to generate line plots and histograms of any expression
- History search system
- Advanced autocomplete for GDScript and custom commands
- Async-compatible (supports awaiting signals)
- Remote console, for running commands remotely (potentially from another device)
- High performance (since it's written in Rust)
- Log hook support (see section [Log hook](../40_misc/09_log_hook.md))
- VR support (experimental - see section [VR support](../40_misc/11_vr.md))
- Fully open source
- Lightweight: small in filesize, uses little CPU and RAM 
- AI-free documentation: no LLM output is used in this documentation, it is 100% written by hand

## License

CrabbyConsole is licensed under either MIT or Apache 2.0, whichever you prefer[^1].

[^1]: This is the [recommended license](https://rust-lang.github.io/api-guidelines/necessities.html#crate-and-its-dependencies-have-a-permissive-license-c-permissive) for Rust crates.