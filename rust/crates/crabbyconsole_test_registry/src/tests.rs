//! Put all your integration tests here! Don't forget to annotate them with `#[godot_test]`.

use std::{
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
    thread,
    time::Duration,
};

use crabbyconsole_core::gd::console::CrabConsole;
use crabbyconsole_integration_tests_util::godot_version;
use crabbyconsole_macros::godot_test;
use crabbyconsole_misc::util::{get_current_scene, get_root, get_scene_tree, get_viewport};
use godot::{
    classes::{Camera2D, Camera3D, GDScript, ImageTexture, Time, Viewport, Window},
    global::Error as GodotError,
    obj::Singleton as _,
    prelude::*,
    tools::get_autoload_by_name,
};
use rand::{Rng, seq::SliceRandom};

/// Get the CrabbyConsole
fn console() -> Gd<CrabConsole> {
    // This should be super fast, it caches the autoload
    get_autoload_by_name::<CrabConsole>("CrabbyConsole")
}

/// Evaluate an expression via CrabbyConsole
fn eval(expr: &str) -> Variant {
    CrabConsole::eval_blocking(console(), expr.into())
}

// Add a custom command to CrabbyConsole
fn add_custom_command<F, R>(name: &str, f: F) -> GodotError
where
    R: ToGodot,
    F: 'static + Send + Sync + FnMut(&[&Variant]) -> R,
{
    CrabConsole::add_custom_command(
        console(),
        name.into(),
        Callable::from_sync_fn("", f),
        "".into(),
    )
}

#[godot_test]
fn this_should_not_panic() {
    // `Expression` class does not work here, it returns ERR_INVALID_PARAMETER (Expected '(')

    let native_class = {
        //  ClassDb::singleton().instantiate("GDScriptNativeClass"),
        // ...does not work, returns `null`

        let mut script = GDScript::new_gd();
        script.set_source_code(
            r#"
extends RefCounted

func foobar_func():
    return Object
        "#,
        );
        script.reload().into_result().unwrap();
        let mut obj = RefCounted::new_gd();
        obj.set_script(&script);
        obj.try_call("foobar_func", &[])
    };

    // Type = GDScriptNativeClass
    eprintln!(
        "Result type: {:?}",
        native_class
            .as_ref()
            .map(|v| { v.to::<Gd<Object>>().get_class() })
    );

    // We expect a panic until `godot` fixes this. Hopefully fixed in godot 0.5.6?
    let result = catch_unwind(AssertUnwindSafe(|| {
        eprintln!("Result: {:?}", native_class); // <-- panic here
    }));
    let result_mapped = result
        .as_ref()
        .map_err(|err| err.downcast_ref::<String>().unwrap());
    assert_eq!(
        result_mapped,
        Err(&"Function call failed:  call -- method not found.".to_owned()),
        "expected panic in Debug impl of GDScriptNativeClass: {:#?}",
        result_mapped
    );

    // You can see it panics during formatting, because the output looks garbled:
    /*
        Result type: Ok("GDScriptNativeClass")
        Result: Ok(The application panicked (crashed).
        Message:  Function call failed:  call -- method not found.
    */
}

#[godot_test]
fn basic_tests() {
    // Check if the console is in a working state at all.
    assert_eq!(eval("1+2").to::<i32>(), 3);

    // Check if the console singleton even exists and points to the right object
    assert_eq!(
        eval(":node find CrabbyConsole").to::<Gd<CrabConsole>>(),
        console()
    );
}

#[godot_test]
fn sleep_tests() {
    let start_time = Time::singleton().get_ticks_usec();
    eval(":asleep 1");
    let elapsed_usec = Time::singleton().get_ticks_usec() - start_time;
    let elapsed_msec = elapsed_usec as f64 / 1000.0;

    // Check if :asleep 1 sleeps for at least one second.
    assert!(elapsed_msec > 1000.);
}

#[godot_test]
fn variable_tests() {
    // Check if variables work properly.
    assert_eq!(eval(":set x 7").to::<i32>(), 7);
    assert_eq!(eval(":set y 2").to::<i32>(), 2);
    assert_eq!(eval(":set z x+y").to::<i32>(), 9);

    assert_eq!(eval(":get x").to::<i32>(), 7);
    assert_eq!(eval(":get z").to::<i32>(), 9);

    assert!(eval(":set y null").is_nil());
    assert!(eval(":get y").is_nil());

    assert_eq!(
        eval(":vars").to::<Array<VarArray>>(),
        array![
            &varray![&StringName::from("x"), 7],
            &varray![&StringName::from("z"), 9]
        ]
    ); // &"x" indicates a StringName
}

#[godot_test]
fn variable_invalid_tests() {
    // Check if invalid variable names are rejected
    let set_invalid = |name: &str| {
        eprintln!("variable name: `{name}`");
        assert!(eval(&format!(":set {name} 7")).is_nil());
        assert!(eval(&format!(":get {name}")).is_nil());

        let result = eval(&format!("{name} + 1"));
        assert!(
            result.is_nil(),
            "expected `{name} + 1` to return nil, but it returned `{result}`"
        );
    };

    set_invalid("-invalid"); // cannot start with a dash
    set_invalid("-- -invalid"); // need this to pass `-invalid` into clap, else it gets interpreted as just `-i`
    set_invalid("0x"); // cannot start with a number
    set_invalid("'");
    set_invalid("don't");
    set_invalid("a b");
    set_invalid("@");
    set_invalid("#");
    set_invalid("!axe");
    set_invalid("~axe");
    set_invalid(":set");
    set_invalid("a^b");
    set_invalid("(");
    set_invalid(")");
    set_invalid("[");
    set_invalid("]");
    set_invalid("{");
    set_invalid("}");

    // reserved GDScript keywords
    set_invalid("var");
    set_invalid("break");
    set_invalid("for");
    set_invalid("if");

    // Can't test these...
    // set_invalid("0");  // triggers false positive (0 + 1 == 1)
    // set_invalid(" ");  // triggers false positive ( + 1 == 1)
    // set_invalid("!");  // triggers false positive (! + 1 == false)
    // set_invalid("~");  // triggers false positive (! + 1 == -1)

    // Expect no variables to be set at all
    assert_eq!(eval(":vars").to::<Array<VarArray>>(), array![]);
}

#[godot_test]
fn variable_stress_tests() {
    // This test creates a large amount of variables in a random order.
    // It checks if they have the expected values, and finally unsets them.
    // According to samply, this test spends 90% of the time in GDScript::reload.

    let set = |name: &str, value: i32| {
        eprintln!("setting `{name}` to `{value}`");

        assert_eq!(eval(&format!(":set {name} {value}")).to::<i32>(), value);
    };

    let set_null = |name: &str| {
        eprintln!("setting `{name}` to `null`");

        assert!(eval(&format!(":set {name} null")).is_nil());
    };

    let get = |name: &str, value: i32| {
        eprintln!("getting `{name}` and checking if it equals `{value}`...");

        assert_eq!(eval(&format!(":get {name}")).to::<i32>(), value);

        let result = eval(&format!("{name} + 1")).to::<i32>();
        assert_eq!(result, value + 1);
    };

    let get_null = |name: &str| {
        eprintln!("getting `{name}` and checking if it equals `nil`...");

        assert!(eval(&format!(":get {name}")).is_nil());
        assert!(eval(&format!("{name} + 1")).is_nil());
    };

    let mut vars = (0..700) // 1000 = super slow (66s)
        .map(|i| (format!("var_{i}"), i))
        .collect::<Vec<_>>();

    // Shuffle
    let mut rng = rand::rng();
    vars.shuffle(&mut rng);

    // Set all vars and check their values
    for (name, value) in &vars {
        set(name, *value);
        get(name, *value);
    }

    // Check if all variables were set and are in the correct order
    assert_eq!(
        eval(":vars").to::<Array<VarArray>>(),
        vars.iter()
            .map(|(k, v)| { varray![&StringName::from(k), *v] })
            .collect::<Array<VarArray>>()
    );

    // Shuffle again
    vars.shuffle(&mut rng);

    // Check all values again and increment them
    for (name, value) in &vars {
        get(name, *value);

        set(name, *value + 1);
        get(name, *value + 1);
    }

    // Check if all variables were set, but this time don't check the order (it retains insertion order from before the second shuffle)
    assert_eq!(
        sorted(
            &eval(":vars")
                .to::<Array<VarArray>>()
                .iter_shared()
                .map(|arr| (
                    arr.at(0).to::<StringName>().to_string(), // Convert StringName->String, since StringName != Ord
                    arr.at(1).to::<i32>()
                ))
                .collect::<Vec<_>>()
        ),
        sorted(
            &vars
                .iter()
                .map(|(k, v)| { (k.clone(), *v + 1) }) // +1 since we just incremented them
                .collect::<Vec<_>>()
        ),
    );

    // Shuffle for the third time
    vars.shuffle(&mut rng);

    // Now check all vars again and then unset them
    for (name, value) in &vars {
        get(name, *value + 1);
        set_null(name);
    }

    // Shuffle for the last time
    vars.shuffle(&mut rng);

    // Now check if they're all null
    for (name, _) in &vars {
        get_null(name);
    }

    // Expect no variables to be set at all now
    assert_eq!(eval(":vars").to::<Array<VarArray>>(), array![]);
}

#[godot_test]
fn for_tests() {
    // Check if for loops work properly.
    assert_eq!(
        eval(":for i 1 6 {i}").to::<VarArray>(),
        varray![1, 2, 3, 4, 5, 6]
    );
    assert_eq!(
        eval(":for i 1 6 --step 2 {i}").to::<VarArray>(),
        varray![1, 3, 5]
    )
}

#[godot_test]
fn seq_par_tests() {
    // Check if :seq and :par work properly.
    assert_eq!(eval(":seq 5 |> 6 |> 7").to::<i32>(), 7);
    assert_eq!(eval(":seq :set x 10 |> :set y 9 |> x+y").to::<i32>(), 19);
    assert!(eval(":seq :set m :perf mem |> m > 1").to::<bool>()); // assume we use more than 1MB of ram

    assert_eq!(eval(":par 6 <> 7 <> 8").to::<VarArray>(), varray![6, 7, 8]);
}

#[godot_test]
fn cmd_tests() {
    // Check if :cmd and add_custom_command work properly.

    ////

    assert_eq!(add_custom_command("foobar", |_| 1 + 2), GodotError::OK);
    assert_eq!(eval(":foobar").to::<i32>(), 3);

    ////

    assert_eq!(
        add_custom_command("concat", |ab| {
            let [a, b] = ab else {
                panic!("expected two args");
            };

            a.evaluate(b, VariantOperator::ADD).unwrap()
        }),
        GodotError::OK
    );
    assert_eq!(eval(":concat 1 2").to::<String>(), "12");

    ////

    assert_eq!(
        add_custom_command("gather", |ab| {
            VarArray::from_iter(ab.iter().cloned().cloned()) // need 2 clones here to go &&Variant -> &Variant -> Variant
        }),
        GodotError::OK
    );
    assert_eq!(eval(":gather 1 2").to::<VarArray>(), varray!["1", "2"]);

    ////

    // invalid command: invalid names
    assert_ne!(add_custom_command("-invalid", |_| 1 + 2), GodotError::OK);
    assert!(eval(":-invalid").is_nil());
    assert_ne!(add_custom_command("$cash", |_| 1 + 2), GodotError::OK);
    assert!(eval(":$cash").is_nil());

    // invalid command: reserved names
    assert_ne!(add_custom_command("quit", |_| 1 + 2), GodotError::OK);
    assert_ne!(add_custom_command("help", |_| 1 + 2), GodotError::OK);
    assert!(eval(":help").is_nil()); // Counter-intuitively, :help does not return the help string; it returns a clap error, which gets turned into nil when passing to Godot
}

#[godot_test]
fn delta_tests() {
    // Check if :delta works properly.

    assert_eq!(eval(":set x 3").to::<i32>(), 3);
    assert_eq!(eval(":delta x").to::<i32>(), 0);

    assert_eq!(eval(":set x 4").to::<i32>(), 4);
    assert_eq!(eval(":delta x").to::<i32>(), 1); // 4 - 3

    assert_eq!(eval(":set x 0").to::<i32>(), 0);
    assert_eq!(eval(":delta x").to::<i32>(), -4); // 0 - 4
}

#[godot_test]
fn crash_tests() {
    // this used to cause a segfault, prevented by SafeDebug now
    eval(":set sumfunc func(a, b): return a + b");
    eval("CrabbyConsole.add_custom_command.call_deferred(\"sum\", sumfunc)");
    eval("sumfunc");

    // this should also no longer crash
    eval(":set sumfunc2 func(a, b): return a + b");
    eval("CrabbyConsole.add_custom_command(\"sum2\", sumfunc2)");

    // this used to panic, then we "fixed" it by making it return null, but now it's ACTUALLY fixed and returns the actual GDScriptNativeClass without panicking
    eval(":set obj Object");
    assert!(eval("obj == Object").to::<bool>());
    // Warning: if you call eval_blocking recursively, you get: cannot execute `LocalPool` executor from within another executor: EnterError

    eval(":set res Resource");
    assert!(eval("res == Resource").to::<bool>());
}

#[godot_test]
fn tokio_tests() {
    // Check if Tokio is integrated properly + :par works + :eval-file works.

    // this test is quite slow (~55s) -  doing this multiple times in case it's non-deterministic
    for i in 0..10 {
        println!("tokio test iteration {i}");
        assert!(
            eval(":par :debug allocate 99999999 <> :debug allocate 99999999")
                .to::<VarArray>()
                .iter_shared()
                .all(|s| s.to::<String>().contains("Allocated"))
        );

        //  running these in parallel triggers the TOKIO_RUNTIME.enter() panic bug
        let evaluated = eval(
            ":par :eval-file addons/crabbyconsole/scripts/test/test_commands.txt <> :eval-file addons/crabbyconsole/scripts/test/test_commands.txt",
        );
        assert!(
            evaluated
                .to::<VarArray>()
                .iter_shared()
                .all(|s| s.to::<String>().contains("Evaluated"))
        );
        assert!(eval("load_from_file_successful").to::<bool>());
    }
}

#[godot_test]
fn lockdown_test() {
    // Check if lockdown mode works properly.

    assert_eq!(eval("1+2").to::<i32>(), 3); // gdscript eval should not fail
    assert!(!eval("1+2").is_nil()); // gdscript eval should not be nil
    assert!(CrabConsole::lockdown().to::<String>().contains("enabled")); // enter lockdown mode
    assert!(eval("1+2").is_nil()); // gdscript eval should fail now
}

#[godot_test]
fn sysinfo_test() {
    // Checks if :sysinfo returns the correct godot version.
    // godot_version()       = "4.6.2.stable.official.<build_id>"
    // godot_runtime_version = "4.6.2.stable.official"

    let a = godot_version().expect("failed to get godot version");
    let b = {
        let godot_runtime_version = eval(":sysinfo")
            .to::<AnyDictionary>()
            .at("engine")
            .to::<AnyDictionary>()
            .at("godot_runtime_version")
            .to::<String>();
        godot_runtime_version
            .strip_prefix("v") // Little hack, since it puts "v" in front of the version
            .map(String::from)
            .expect("expected `godot_runtime_version` to start with `v`")
    };

    assert!(a.starts_with(&b), "expected `{a}` to start with `{b}`");
}

#[godot_test]
fn mem_leak_test() {
    // We currently have a small mem leak in the gdscript evaluator, this checks to ensure it doesn't regress to become worse.
    // Once you add caching to the gdscript evaluator it should become much better.

    let expr = "randf()";

    // Warmup (to initialize lazily evaluated things BEFORE measuring memory usage)
    let warmup_max_i: usize = 10000;
    for i in 0..warmup_max_i {
        if i.is_multiple_of(1_000) {
            eprintln!("warmup {i}/{warmup_max_i}");
        }
        eval(expr);
    }

    let mem_before = eval(":perf mem").to::<f64>();

    //eval 100_000x = +28.539 MB
    //eval 1_000_000x = +276.87109375 MB
    // -> so we leak 0.0002768710937 MB = 290 bytes per command. Interesting!

    // If you evaluate the expression "randf()", it gets converted into this GDscript code:

    /*
    extends CrabConsoleScriptContext

    func run_12447488989728999609():
        @warning_ignore_start("unused_variable")
        @warning_ignore_start("unused_parameter")
        @warning_ignore_start("return_value_discarded")
        @warning_ignore_start("shadowed_variable")
        @warning_ignore_start("integer_division")

        return randf()
    */

    let max_leak_mb_per_cmd = 350.0 / 1024.0 / 1024.0; // 310 = too low

    // Test for memory leaks in the GDScript evaluator
    let max_i: usize = 100_000;
    for i in 0..max_i {
        if i.is_multiple_of(10_000) {
            eprintln!("iteration {i}/{max_i}");
        }
        eval(expr);
    }
    let mem_after = eval(":perf mem").to::<f64>();

    let max_leak_total = max_i as f64 * max_leak_mb_per_cmd;
    let mem_diff = mem_after - mem_before;

    dbg!(mem_before);
    dbg!(mem_after);
    dbg!(mem_diff);
    dbg!(mem_diff / max_i as f64);
    dbg!(max_leak_mb_per_cmd);
    dbg!(max_leak_total);
    dbg!(mem_diff < max_leak_total);
    assert!(
        mem_diff < max_leak_total,
        "Memory usage went from {mem_before} MB -> {mem_after} MB after running {max_i} GDScript commands, a difference of {mem_diff} MB. \
        The difference was too big, it should've been at most {max_leak_total} MB",
    );
}

#[godot_test]
fn profile_test() {
    // Check if :profile works properly.

    for _ in 0..50 {
        // You can verify the min/max time spent by doing: :watch add p :plot line -t 30 :profile :sleep 0.5
        let time_spent = eval(":profile :sleep 0.5").to::<f64>();
        assert!(
            (0.5..0.7).contains(&time_spent),
            "expected `:sleep 0.5` to take about 0.5s, but it took {time_spent}s"
        );
    }
}

#[godot_test]
fn plot_test() {
    // Check if :plot line caching works properly.

    let plot_types = ["line", "histo"];
    let exprs = vec!["1+2", "randf()", "Time.get_ticks_msec()"];
    let mut all_plots = vec![];

    for plot_type in plot_types {
        let mut plots = vec![];

        for expr in &exprs {
            let final_expr = &format!(":plot {plot_type} {expr}");
            dbg!(final_expr);

            let plot_original = eval(final_expr).to::<Gd<ImageTexture>>();

            let plot_img = plot_original.get_image().expect("plot has no image");
            assert_ne!(plot_img.get_width(), 0);
            assert_ne!(plot_img.get_height(), 0);

            plots.push(plot_original.clone()); // fast clone
            all_plots.push(plot_original.clone()); // fast clone

            // Cache test: generating the same plot on the same expression should cache the texture, not generate a new one every time
            for _ in 0..100 {
                let plot = eval(final_expr).to::<Gd<ImageTexture>>();

                // Note - this checks for instance id equality, it doesn't check the images per-pixel
                assert_eq!(plot_original, plot);
            }
        }

        // Check that all plot textures are unique, so no two expressions get the same plot
        dbg!(&plots);
        let mut seen = HashSet::new();
        assert!(
            plots.iter().all(|p| seen.insert(p)),
            "plot textures weren't unique across different expressions"
        );
    }

    // Check that all plot textures are unique, so no two different plot types get the same plot
    // We don't want a histogram and a line plot using the same texture, since they will overwrite each other and fight over the pixels.

    dbg!(&all_plots);
    let mut seen = HashSet::new();
    assert!(
        all_plots.iter().all(|p| seen.insert(p)),
        "plot textures weren't unique across different plot types"
    );
}

#[godot_test]
fn plot_nan_test() {
    // Fixed panic or infinite loop if you feed inf/nan into :plot, keep this test around to prevent regressions.

    let values = [
        "1.0 / 0.0",             // = inf
        "1.0 / 0.0 - 1.0 / 0.0", // = nan
    ];

    let plot_types = ["line", "histo"];

    for plot_type in plot_types {
        for value in values {
            for _ in 0..10 {
                let expr = format!(":plot {plot_type} {value}");
                dbg!(&expr);

                // Generate first plot
                let plot1 = eval(&expr).to::<Gd<ImageTexture>>();

                // Important - wait a bit, otherwise it doesn't trigger the panic
                thread::sleep(Duration::from_secs(1));

                // Generate second plot - tt should not fail, because it detects y_diff being NaN (inf - inf = NaN) and uses a default x_range and y_range
                let plot2 = eval(&expr).to::<Gd<ImageTexture>>();

                // They should point to the same texture, thanks to the plot texture cache
                assert_eq!(plot1, plot2);
            }
        }
    }
}

#[godot_test]
fn mem_test() {
    // Checks if :perf mem actually gives accurate results.

    // Warmup
    thread::sleep(Duration::from_secs(1));

    // Check mem usage
    let mem_before = eval(":perf mem --bytes").to::<f64>();

    // Allocate a 100MB buffer
    let byte_count = 100 * 1024 * 1024;
    let mut buffer = vec![0u8; byte_count];

    // Fill it with random data, to ensure it actually gets allocated
    let mut rng = rand::rng();
    rng.fill_bytes(&mut buffer);

    // Give some time to update the value
    thread::sleep(Duration::from_secs(1));

    // Check mem usage again
    let mem_after = eval(":perf mem --bytes").to::<f64>();
    let mem_diff = mem_after - mem_before;

    // It should be roughly 100MB higher now with a 10% margin (could be slightly off due to stuff happening in the background).
    // In my tests it seems to be 0.02% off at most, but it could be way higher depending on which Godot project you run this in.
    // E.g. there could be an autoload doing stuff in the background. So let's keep it generous.
    dbg!(mem_before, mem_after, mem_diff, byte_count);
    assert!(
        within_percent(mem_after, mem_before + byte_count as f64, 10.0),
        "expected memory usage to go up by roughly {byte_count} bytes, but it went up by {} bytes",
        mem_after - mem_before
    );

    drop(buffer);
}

#[godot_test]
fn gdscript_semicolon_test() {
    // Check if the semicolon splitter in the GDScript evaluator works properly.

    // Run this multiple times, to ensure it doesn't pollute the environment.
    // (if it does get polluted, the test will fail with `There is already a variable named "a" declared in this scope.`)
    for _ in 0..10 {
        assert_eq!(eval("var a = 1; a += 1; a").to::<i32>(), 2);
        assert_eq!(eval("var a = 3; var b = 4; a + b").to::<i32>(), 7);
    }
}

#[godot_test]
fn gdscript_custom_command_callable_bug() {
    // There is currently a bug. If you do this:
    eval(r#"CrabbyConsole.add_custom_command("baz", func(): return 7 + 2)"#);

    // If you run it directly afterwards, it works fine.
    assert_eq!(eval(":baz").to::<i32>(), 9);

    // However, if you evaluate another GDScript command afterwards...
    eval("1+2");

    // ... the Callable gets invalidated.
    assert!(eval(":baz").is_nil());

    // So, currently we expect this to fail. Once you fix the bug, reverse the above check and assert :baz == 9.
}

#[godot_test]
fn gdscript_control_characters() {
    // Check if control characters are filtered out in the GDScript evaluator.
    assert_eq!(eval("5 + \n 6").to::<i32>(), 11);
    assert_eq!(eval("5 + \r\n 6").to::<i32>(), 11);
    assert_eq!(eval("5 + \t 6").to::<i32>(), 11);
    assert_eq!(eval("5 + \0 6").to::<i32>(), 11);

    // More thorough check. This gets all Unicode control characters
    fn all_control_chars() -> Vec<char> {
        (0x00..=0x1F)
            .chain(0x7F..=0x9F)
            .filter_map(char::from_u32)
            .collect()
    }

    let control_chars = all_control_chars();
    assert_eq!(control_chars.len(), 65);

    for c in all_control_chars() {
        assert_eq!(
            eval(&format!("10 {c} + 20")).to::<i32>(),
            30,
            "failed to strip control character U+{:04X} -> {:?}",
            c as u32,
            c
        );
        assert_eq!(
            eval(&format!("10 {c} + {c} 20")).to::<i32>(),
            30,
            "failed to strip control character U+{:04X} -> {:?}",
            c as u32,
            c
        );
    }
}

#[godot_test]
fn gdscript_for_loop1() {
    // Check if we can make a inline for loop.
    // Note - for mutable variables, use arrays, NOT integers.
    // If you try to mutate them nothing happens, since they're not reference types.

    assert_eq!(eval(r#":set hello_chars []"#).to::<VarArray>(), varray![]);
    // Check both with and without trailing semicolon.
    assert!(eval(r#"for c in "hello": hello_chars.push_back(c)"#).is_nil()); // for loops return nil
    assert!(eval(r#"for c in "_there": hello_chars.push_back(c);"#).is_nil()); // for loops return nil
    assert_eq!(
        eval(r#" "".join(hello_chars)"#).to::<String>(),
        "hello_there",
    );
}

#[godot_test]
fn gdscript_for_loop2() {
    // Now check multiple statements per line - does NOT work at the moment.

    assert_eq!(eval(r#":set bye_chars []"#).to::<VarArray>(), varray![]);
    // Check both with and without trailing semicolon.

    assert!(eval(r#"for c in "bye": bye_chars.push_back(c); bye_chars.push_back(c)"#).is_nil());
    assert!(eval(r#"for c in "bye": bye_chars.push_back(c); bye_chars.push_back(c);"#).is_nil());

    // Does not work at the moment, so expect empty string
    assert_eq!(eval(r#" "".join(bye_chars)"#).to::<String>(), "",);
}

#[godot_test]
fn gdscript_for_loop3() {
    // Now check statements AFTER a for loop.

    assert_eq!(eval(r#":set arr []"#).to::<VarArray>(), varray![]);

    // Check both with and without trailing semicolon.
    assert!(eval(r#"arr.push_back(1)"#).is_nil());
    assert!(eval(r#"for j in 5: print(j); arr.push_back(1)"#).is_nil());
    assert!(eval(r#"for j in 5: print(j); arr.push_back(1);"#).is_nil());

    // There should be only three 1s in the array, not eleven.
    // Since the statement after the for loop is not part of the for loop itself.
    // Kind of misleading! This is not how GDScript works officially, so may want to change this later.
    assert_eq!(eval(r#"arr"#).to::<VarArray>(), varray![1, 1, 1]);
}

#[godot_test]
fn gdscript_for_loop5() {
    // Now check a nested for loop

    assert_eq!(eval(r#":set pairs []"#).to::<VarArray>(), varray![]);
    assert!(eval(r#"for x in 3: for y in 2: pairs.push_back([x, y])"#).is_nil());
    assert_eq!(
        eval(r#"pairs"#).to::<VarArray>(),
        varray![
            &varray![0, 0],
            &varray![0, 1],
            &varray![1, 0],
            &varray![1, 1],
            &varray![2, 0],
            &varray![2, 1],
        ],
    );
}

#[godot_test]
fn gdscript_semicolon_if() {
    // Expect both statements to run.
    assert_eq!(eval(r#":set arr1 []"#).to::<VarArray>(), varray![]);
    assert!(eval(r#"if true: arr1.append("a"); arr1.append("b")"#).is_nil());
    assert_eq!(eval(r#"arr1"#).to::<VarArray>(), varray!["a", "b"]);

    // Expect neither statement to run -> actually for now `b` does run, will be fixed after adding the new evaluator.
    // This is confusing since GDScript does the opposite.
    assert_eq!(eval(r#":set arr2 []"#).to::<VarArray>(), varray![]);
    assert!(eval(r#"if false: arr2.append("a"); arr2.append("b")"#).is_nil(),);
    assert_eq!(eval(r#"arr2"#).to::<VarArray>(), varray!["b"]);
}

#[godot_test]
fn gdscript_ternary_if() {
    // Checks if the ternary operator works.

    assert_eq!(eval("5 if true else 4").to::<i32>(), 5);
    assert_eq!(eval("5 if false else 4").to::<i32>(), 4);

    eval(":set foo true");
    assert_eq!(eval("8 if foo else 7").to::<i32>(), 8);
    eval(":set foo false");
    assert_eq!(eval("8 if foo else 7").to::<i32>(), 7);
}

// Looking at Godot's source code, it only explicitly accepts \n and \r\n as newline characters.
// Any other weird variants are rejected. \r alone is also rejected (gives "Stray carriage return character in source code.").
// So that seems fine and dandy.

#[godot_test]
fn gdscript_pathological_case1() {
    // test case that causes a crash in defer!:
    // > free(): double free detected in tcache 2

    /* this generates the following code:

    func run_8427805413010771541():
            @warning_ignore_start("unused_variable")
            @warning_ignore_start("unused_parameter")
            @warning_ignore_start("return_value_discarded")
            @warning_ignore_start("shadowed_variable")
            @warning_ignore_start("integer_division")

            pass
    func _init():
            set_script(null)

    */

    // it seems impossible to actually generate this though, since you can't type actual newlines in a single-line LineEdit.
    // also, we filter out all newlines anyway
    let expr = "pass\nfunc _init():\n\tset_script(null)";
    assert!(eval(expr).is_nil());

    // Now assert the GDScript evaluator still works at all.
    assert_eq!(eval(r#"1+2"#).to::<i32>(), 3);
}

#[godot_test]
fn gdscript_pathological_case2() {
    // works fine, no crash

    let expr = "pass\nfunc _init():\n\tqueue_free()";
    assert!(eval(expr).is_nil());

    // or code = "pass\nfunc _init():\n\tcall_deferred(\"free\")";

    // Now assert the GDScript evaluator still works at all.
    assert_eq!(eval(r#"1+2"#).to::<i32>(), 3);
}

#[godot_test]
fn gdscript_pathological_case3() {
    // caught by godot: Invalid white space character U+2028

    //pass<U+2028>func _init():<U+2028>    set_script(null)
    let expr = "pass\u{2028}func _init():\u{2028}    set_script(null)";
    assert!(eval(expr).is_nil());

    // Now assert the GDScript evaluator still works at all.
    assert_eq!(eval(r#"1+2"#).to::<i32>(), 3);
}

#[godot_test]
fn gdscript_pathological_case4() {
    // caught by godot: Stray carriage return character in source code.

    let expr = "pass\rfunc _init():\r    set_script(null)";
    assert!(eval(expr).is_nil());

    // Now assert the GDScript evaluator still works at all.
    assert_eq!(eval(r#"1+2"#).to::<i32>(), 3);
}

#[godot_test]
fn gdscript_built_in_methods1() {
    // Test if all built-in GDScript methods work.

    // Move to dummy scene, otherwise scene() panics.
    // By default there is no main scene if you run godot with the -s flag.
    get_scene_tree().change_scene_to_node(&{
        let mut node = Node::new_alloc();
        node.set_name("awesome_scene");
        node
    });
    // Wait for the scene change to propagate.
    thread::sleep(Duration::from_secs(1));
    // Well that didn't work. We need an actual async await so we can wait for the next frame.
    // Else, scene() panics with "no current scene"
    // TODO go add async tests, then add a one frame await here. Then set async_tests to true:
    let async_tests = false;
    if async_tests {
        assert_eq!(
            eval(r#"scene()"#).to::<Gd<Node>>(), // scene() == null?
            get_current_scene().expect("no current scene")
        );

        assert_eq!(eval(r#"scene().name"#).to::<String>(), "awesome_scene");
    }
    assert_eq!(eval(r#"tree()"#).to::<Gd<SceneTree>>(), get_scene_tree());
    assert_eq!(eval(r#"root()"#).to::<Gd<Window>>(), get_root());
    assert_eq!(eval(r#"viewport()"#).to::<Gd<Viewport>>(), get_viewport());
    assert_eq!(
        eval(r#"autoload("CrabbyConsole")"#).to::<Gd<CrabConsole>>(),
        console()
    );

    // Test absolute path.
    assert_eq!(
        eval(r#"get_node("/root/CrabbyConsole")"#).to::<Gd<CrabConsole>>(),
        console()
    );
    // Test relative path (get_node() is now relative to /root/).
    assert_eq!(
        eval(r#"get_node("CrabbyConsole")"#).to::<Gd<CrabConsole>>(),
        console()
    );

    // There are no cameras initially.
    assert!(eval(r#"cam_2d()"#).is_nil());
    assert!(eval(r#"cam_3d()"#).is_nil());

    // Now let's add some cameras.
    eval(r#"var camA = Camera2D.new(); camA.name="camA"; add_child(camA)"#);
    eval(r#"var camB = Camera3D.new(); camB.name="camB"; add_child(camB)"#);
    assert_eq!(eval(r#"cam_2d()"#).to::<Gd<Camera2D>>().get_name(), "camA");
    assert_eq!(eval(r#"cam_3d()"#).to::<Gd<Camera3D>>().get_name(), "camB");

    // The other methods seem trivial, not worth testing
}
#[godot_test]
fn gdscript_built_in_methods2() {
    // Check if add_child(node) works. It should add node to the current main scene... except we don't have one, so it adds to root() instead.
    let instance_id = eval(r#"var node = Node.new(); node.name = "foobar123"; add_child(node); node.get_instance_id()"#).to::<InstanceId>();
    assert_eq!(
        eval(r#"root().get_node("foobar123").get_instance_id()"#).to::<InstanceId>(),
        instance_id
    );

    // Now move to a real scene and try it again
    // TODO we can only add that once we have async testing, since we need to await at least one frame for the scene switch to take effect.
}

/// Checks if `a` is within `percent%` of `b`.
fn within_percent(a: f64, b: f64, percent: f64) -> bool {
    if a == b {
        return true;
    }
    let diff = (a - b).abs();
    let largest = a.abs().max(b.abs());
    diff / largest <= percent / 100.0
}

/// Use this to assert_eq! two Vecs, except order doesn't matter.
/// Note - do not use HashSet, it silently drops duplicates
fn sorted<T: Ord + Clone>(v: &[T]) -> Vec<T> {
    let mut v = v.to_vec();
    v.sort();
    v
}
