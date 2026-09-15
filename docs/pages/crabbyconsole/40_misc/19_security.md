---
icon: lucide/shield-check
---

# Security

This tool is made for your own use only. Do not let untrusted users run commands, e.g. by exposing the remote console to the open internet. Commands can run arbitrary unsandboxed GDScript, so they can do anything - ranging from deleting files to downloading and executing arbitrary executables. Fixing this would require Godot to support sandboxed GDScript execution, so that's beyond the scope of this project.

## Lockdown mode

To mitigate the risk, you can enter lockdown mode by running this somewhere in your game:

```gdscript title="GDScript"
CrabbyConsole.lockdown()
```

From that point onwards, GDScript execution will be disallowed in the console; you can only run (non-risky) Clap commands.

!!! tip "Protip"

    Once lockdown mode is enabled, it cannot be turned off again.

Try running this in the console now:

```gdscript
1+2
```

Notice how it doesn't print `3`, instead it prints an error. This is because `1+2` is interpreted as GDScript.

### Recursivity
Lockdown mode is applied recursively. That means, if a Clap command runs GDScript, it will also fail:
```
:set a 1+2
```
Even though `:set` is a Clap command, this will still fail, because it has to evaluate `1+2` as GDScript before it can assign it to variable `a`.

However, a Clap command evaluating another Clap command *is* allowed:

```
:set a :win size
```

Now `a` contains the current window size.

To get the value of `a`, use:

```
:get a
```
...because if you would have written just `a`, it would be interpreted as GDScript, so it would fail.

!!! tip "Protip 2"

    Custom GDScript commands will continue to work in lockdown mode.

### Risky commands

Some Clap commands are marked as risky, which means they cannot be used in lockdown mode. To list all risky commands, you can search the Command Reference for the word "risky".