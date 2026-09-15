---
icon: lucide/terminal
---

# Running commands

There are two kinds of commands:

**Clap commands**
: A Clap command always starts with `:`, such as `:reload`, `:restart`, etc. These are commands that are built into the console to help you perform common actions. You can also add custom ones yourself, see section [Adding custom commands](../20_commands/10_custom_commands.md).

**GDScript commands**
: These are commands that consist of plain GDScript. It can run basically anything. It can  access every node and resource in your game, access any property and call any method on them. It can also access every singleton and class, such as `Engine` and `OS`. This is very powerful, but can also be dangerous; see [Security](../40_misc/19_security.md) section to learn more about mitigating the risk.


## Example commands

Try running some of these to get started:

### Set window size
```
:win size 1280 800
```

### Enable fullscreen
```
:win mode fullscreen
```

### Toggle VSync
```
:win vsync toggle
```
Then, you can run `:win vsync` to see if VSync is currently enabled or not.

### Show detailed information about the system
```
:sysinfo --pretty
```

This prints the CPU model, amount of cores/threads, GPU name, GPU driver version, total RAM amount, Godot version, etc. This is useful for tech support, since it can be copy-pasted and sent to developers for troubleshooting.

### See how much memory your game is using (in megabytes)
```
:perf mem 
```
!!! tip "Protip"
    Unlike [`Performance.get_monitor(Performance.MEMORY_STATIC)`](https://docs.godotengine.org/en/stable/classes/class_performance.html#enum-performance-monitor), this works in an exported game as well.


### Reload the current scene
```
:reload
```

### Restart the game
```
:restart
```

### Clear the console
```
:clear
```

### Bind a key to an action
```
:key bind f12 :screenshot 
```
Now you can press ++f12++ to take a screenshot of your game. The screenshot will be saved in `user://screenshots/`, you can open that folder automatically by running:

```
:open screenshots
```

----

Another possibility:

```
:key bind f5 :reload
```
Now you can press ++f5++ to reload the main scene.

----

Finally, try running this:
```linenums="1"
:key bind 1 :speed 1
:key bind 2 :speed 2
:key bind 3 :speed 4
:key bind 4 :speed 0.5
:key bind 5 :speed 0.25
:key bind 0 :pause
```

Now you can:

- Press ++1++ to reset the game speed
- Press ++2++ to double the game speed
- Press ++3++ to quadruple the game speed
- Press ++4++ to halve the game speed (so it runs in slow motion)
- Press ++5++ to quarter the game speed
- Press ++0++ to toggle pause

!!! note
    This uses [`unhandled_input`](https://docs.godotengine.org/en/stable/classes/class_node.html#class-node-private-method-unhandled-input), so it will only work if nothing else in your game is consuming the input events. For example, if you press ++0++ in a text box, it won't work, because the text box already consumed the event. However, function keys like ++f12++ are typically not consumed by text boxes, because they don't type anything if you press them, so they should work fine.

To remove keybinds, you can use `:key unbind`. For example, to unbind the key ++1++, you can do `:key unbind 1`. Alternatively, you can do `:key clear` to unbind all keys.

### Move to another scene
```bash
:load res://main.tscn # (1)!
```

1.  Replace `res://main.tscn` with the name of an actual name of a scene in your game.

### Set framerate limit
```gdscript title="GDScript"
Engine.max_fps = 30
```
Now the game will be capped to 30 frames per second.

### Reduce the 3D render resolution
```gdscript title="GDScript"
viewport().scaling_3d_scale = 0.3
```
Now the game will render at only 30% resolution.

### Enable wireframe mode
```gdscript title="GDScript"
viewport().debug_draw = Viewport.DEBUG_DRAW_WIREFRAME
```
Now your game will be drawn in wireframe mode[^1]. 

### Find nodes by (`NodePath`) pattern
```
:node find --all Foo --path
```
This prints the `NodePath` of all nodes that have `Foo` in their `NodePath`.

### Find all nodes that inherit a specific class
```
:node find --all --type Sprite2D --path
```
This prints the `NodePath` of all `Sprite2D` nodes (and derivatives) in the scene tree.

## Getting help
There are many more possible commands, run `:help` to explore all of them:
```
:help
```
To see how to use a specific subcommand, e.g. `:win`, you can do one of the following:
```linenums="1"
:win help
:help win
:win --help
:win -h
```

For a full list of all commands, see section [API reference](../../api_reference/commands/index.md).

## Using variables

You can use `:set` to set variables to use later, for example:
```linenums="1"
:set foo 1+2
:set bar foo*2
print(foo)
```
Now it prints `6`.

The right side of `:set` can be any command, Clap or GDScript. Example:
```
:set foo :win size
```

Now `foo` contains the current window size.

To unset a variable, set it to `null`:
```linenums="1"
:set foo null
foo
```

Now accessing `foo` will print an error.

## Evaluating multi-line expressions

### GDScript
If you're writing a GDScript command, you can encode multiple lines into the same line by using the `;` character. 

For example:

```gdscript title="GDScript"
var list = [1,2,3]; list.reverse(); list
```

This will return an array `[3, 2, 1]`.

Note that any variables you define in a GDScript command will be lost after the expression is evaluated. If you want to create persistent variables, see section [Using variables](#using-variables).

### Clap
If you're writing a Clap command, you can sequence them using the `:seq` command, which expects commands separated by the `|>` operator, and runs them sequentially:
```
:seq :set foo 1 |> :set bar 2 |> :set baz foo + bar
```

Now `baz` will equal `3`.

Run `:seq --help` to get more details.

### Combining GDScript and Clap

To combine both GDScript and Clap commands, it's generally a good idea to wrap them in `:seq`, since it can handle both GDScript and Clap commands:
```
:seq :set foo 30 |> Engine.max_fps = foo |> Engine.physics_ticks_per_second = foo
```
Now it will set both `Engine.max_fps` and ` Engine.physics_ticks_per_second` to `30`.


## Running commands from a script or file

It may be tedious to keep running the same commands when you startup the game. A better idea is to just run commands from within your own game's scripts:

```gdscript linenums="1" title="GDScript"
CrabbyConsole.eval_blocking(":set foo 1+2")
CrabbyConsole.eval_blocking("print(foo)")
```

???+ warning
    The method is called `eval_blocking()` to warn you of the fact that it blocks the main thread while executing the command. That means it can cause performance problems, or even outright freeze the game, depending on the command. For example, running `CrabbyConsole.eval_blocking(":asleep 5")` will freeze the game for 5 seconds. To prevent the game from locking up forever, the command will be aborted if it takes more than 20 seconds by default. This timeout can be configured by using the `:async timeout` command.

    For more details, see the [Async commands](../30_advanced_commands/15_async.md) section. 

Alternatively, you can use `:eval-file` to evaluate all commands in a file, separated by a newline. To try it, make a text file `commands.txt` in your game's folder, put some commands in it, for example...
```gdscript title="commands.txt"
# print something
print("hello from file")
print("hello again from file")
```
...and then run this:
```
:eval-file commands.txt
```

Lines that start with a `#` will be skipped, which allows you to put comments in the file.

???+ warning
    For security reasons, you can only run `:eval-file` on files inside of your game's directory. Additionally, the command cannot be used in lockdown mode.


[^1]: This only works in a 3D scene; 2D nodes and controls won't be affected. To disable it again, run `viewport().debug_draw = Viewport.DEBUG_DRAW_DISABLED`.

## Built in methods

CrabbyConsole has some built-in GDScript methods you can use, see section [API reference](../../api_reference/gdscript/functions.md) for a full list.
