set positional-arguments # IMPORTANT - otherwise $@ will not work in `test`

# keep this as the first recipe! this is the default recipe that runs - prevents accidental builds if you run just `just`
default:
    just --list

# rwdi = release with debug info
build-rwdi:
    cd rust && cargo build --lib --profile release-with-debug 

# call this in your CI
build-release:
    cd rust && cargo build --lib --profile release 

watch-rwdi:
    cd rust && bacon --job build -- --lib --profile release-with-debug

# this runs both unit tests and integration tests.
# it calls `build-rwdi` first, to ensure CrabConsoleTestRunner is up to date
test-rwdi *args: build-rwdi
    @# TODO maybe depend on `link-so-rwdi` here, so it runs first?
    @# although, we do not want symlinks in CI, they will break if you zip them.
    @# maybe just copy it then.

    @# also, use mold linker: `mold -run just test` -> check `btop`, you should see `mold` in process list instead of `rust-lld`.
    @# we can't put in the justfile yet, need to check on os (it's linux only)

    @# note - do NOT pass --profile here, that means "nextest profile" NOT "cargo profile".
    @# instead, use --cargo-profile.
    @# that seems kind of a waste of compilation time though,
    @# because all the godot test harness is doing is spawning a godot process and getting its stdout.
    @# so there's no point in optimizing it + all its deps.
    @# OTOH, it may share its deps with the main non-test build, since its also in release-with-debug mode,
    @# so it may speed it up regardless. YMMV. 
    @# actually i removed --cargo-profile release-with-debug again, it seems pointless

    @# do NOT use --lib here, it skips the godot tests for some reason
    cd rust && cargo nextest run --no-fail-fast "$@" 

# runs the test in release mode - calls this in your CI
test-release *args: build-release
    # TODO maybe depend on `link-so-release` here?
    cd rust && cargo nextest run --no-fail-fast  "$@" 

# run only unit tests (skips integration tests, so does not require godot to be installed)
test-unit-rwdi:
    just test-rwdi -E 'not binary(godot_test_harness)'        

# Links up the .so file for `release-with-debug` profile, overwriting it if it already exists.
link-so-rwdi:
    mkdir -p "godot/addons/crabbyconsole/bin"
    ln -sf "$(realpath rust/target/release-with-debug/libcrabbyconsole.so)" "godot/addons/crabbyconsole/bin/libcrabbyconsole.so"

# Links up the .so file for `release` profile, overwriting it if it already exists.
link-so-release:
    mkdir -p "godot/addons/crabbyconsole/bin"
    ln -sf "$(realpath rust/target/release/libcrabbyconsole.so)" "godot/addons/crabbyconsole/bin/libcrabbyconsole.so"

# Links up the .so file for `debug` profile, overwriting it if it already exists.
link-so-debug:
    mkdir -p "godot/addons/crabbyconsole/bin"
    ln -sf "$(realpath rust/target/debug/libcrabbyconsole.so)" "godot/addons/crabbyconsole/bin/libcrabbyconsole.so"

# Copies the .so file for `release` profile, overwriting it if it already exists.
copy-so-release:
    mkdir -p "godot/addons/crabbyconsole/bin"
    cp "$(realpath rust/target/release/libcrabbyconsole.so)" "godot/addons/crabbyconsole/bin/libcrabbyconsole.so"

# Run this in your CI
generate-cli-docs-debug:
    cd rust && cargo run --profile debug --bin generate-docs

generate-cli-docs-rwdi:
    # only use --profile release-with-debug if you already built it in that profile, else you're just wasting time
    cd rust && cargo run --profile release-with-debug --bin generate-docs

# Run this in your CI
generate-all-docs:
    # TODO make this task depend on `generate-cli-docs-debug` so that one runs first
    # --strict mode checks for any broken links in your markdown files
    uv run zensical build --strict

# Generate dependency graph and store it in graph.png (requires https://github.com/jplatte/cargo-depgraph)
dep-graph:
    cd rust && cargo depgraph --all-deps --workspace-only | dot -Tpng > ../graph.png

# Measure amount of lines of Rust code across all crates (Linux only for now)
measure-rust-lines:
    @# taken from https://fasterthanli.me/articles/why-is-my-rust-build-so-slow#splitting-into-more-crates
    cd rust && for i in crates/*; do echo "$i" "$(tokei "$i" -o json | jq .Rust.code)"; done
