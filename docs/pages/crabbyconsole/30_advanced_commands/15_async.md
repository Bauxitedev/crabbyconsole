---
icon: lucide/alarm-clock
---

# Async commands

CrabbyConsole supports both sync and async commands.

**Sync commands**
: These are commands that return a result within the same frame. If it takes longer than a frame to calculate the result, it will result in a noticeable stutter or lag spike.

**Async commands**
: These are commands that can take longer to return a result. If the result is not available yet, the executor will yield, so other things can run in the meantime. The executor runs on the main thread[^1], so it's important that it takes up as little time as possible, to avoid causing lag.

So, in total, we support four kinds of commands:

| *Command Type* | Sync | Async
| ------------- | ------------- | -------------
| **Clap**  | :lucide-check:  |  :lucide-check: 
| **GDScript** | :lucide-check: |  :lucide-equal-approximately: [^2]



## Async example

The most basic async command is `:asleep`. It will wait for a given duration asynchronously, so it won't block the main thread while waiting (unlike `:sleep`).

Try it:
```
:asleep 1
```

Now the console will wait one second and then return a result.

Using async building blocks, we can combine async commands together to make more useful things.

### Sequencing
Here's an example of the async building block `:seq`. The syntax is `:seq <A> |> <B>`: it will first run `A`, then `B`.

Try running this:
```
:seq :asleep 1 |> :asleep 2
```
Notice how it waits for 3 seconds, because 1 + 2 = 3.

### Concurrency

Here's an example of the second async building block `:par`. The syntax is `:par <A> <> <B>`: it will run `A` and `B` concurrently [^3].

Try running this:
```
:par :asleep 1 <> :asleep 2
```
Now it only wait for 2 seconds, because during the first second, the two commands were waiting simultaneously.


## Awaiting signals

Now you understand how `:seq` and `:par` work, let's move on to an example that's actually useful in practice.

Rather than waiting for a set time to expire, we can also wait for a signal to be emitted.

Try running:
```
viewport().size_changed
```
Now the console will connect to the `size_changed` signal, and wait for it to be emitted. In other words, it waits until you resize the window.

### Sequencing
Since this happens asynchronously, we can use our `:seq` and `:par` combinators from before.

Try this:
```
:seq viewport().size_changed |> :beep
```
Now it will run the `:beep` command as soon as you resize the window. As you might expect, it plays a beep sound.

!!! tip "Protip"
    Async commands will be cancelled if they take more than 20 seconds by default. So, if you don't resize the window for 20 seconds, it will cancel the beep. This timeout can be configured by using the `:async timeout` command.

Now try this:
```
:seq viewport().size_changed |> root().mouse_exited |> :beep
```
Now it will beep as soon as you 1. resize the window and then 2. move the mouse outside the window. It has to be in that specific order, or it won't work. 

### Concurrency
Same as before, we can use `:par` to await for two things to happen simultaneously.

Try this:
```
:par viewport().size_changed <> root().mouse_exited
```
Now the command will finish if you both 1. resize the window and 2. move the mouse outside the window. However, this time the order doesn't matter, you can also move the mouse first and then resize.

### Sequencing and concurrency

Finally, we can also combine the two:
```
:seq :par viewport().size_changed <> root().mouse_exited |> :beep
```
Now it will beep if you resize the window or move the mouse outside the window, regardless of the order.

## More examples
Remember, this works for **any signal on any node**, so this is is a powerful way to combine signals together to detect patterns of events in complex ways.

Here are some more cool examples of what you can do:
### Awaiting keys

`:key await` is an async command that finishes when the user types any key. You can also give it a key argument, so it only returns when the user types a specific key. For example, `:key await X` only returns as soon as you press ++x++.

Using this command, we can make a command that plays a beep sound when you type "fun":
```
:seq :key await F |> :key await U |> :key await N |> :beep
```
Since we use `:seq`, you have to type ++f++ :lucide-move-right: ++u++ :lucide-move-right: ++n++ in that specific order.

As you might expect, if we add `:par`, the command now allows you to type the letters in any order:
```
:seq :par :key await F <> :key await U <> :key await N |> :beep
```
So ++u++ :lucide-move-right: ++n++ :lucide-move-right: ++f++ will now also play the beep sound.

!!! warning

    If it doesn't work for you, make sure you're not typing in the console's text box. It will consume the text events before `:key await` can read them. Try defocusing the text box and try again.

[^1]: This is because [Godot's scene tree is not thread safe](https://docs.godotengine.org/en/stable/tutorials/performance/thread_safe_apis.html#scene-tree), and since potentially any operation can be performed by running commands in the console, we cannot assume it's safe to run it on another thread.
[^2]: If you call a GDScript signal, the console will await it. On the other hand, if you call an async GDScript method, the console will *not* await it (but it will still run to completion in the background). This behavior is inconsistent, so may be changed later.
[^3]: This is *concurrency*, not *parallelism*. As mentioned before, the async command executor runs on the main thread, so it's *concurrent*, but not *parallel*. In case you don't know the difference between the two terms, more information can be found [here](https://stackoverflow.com/questions/1050222/what-is-the-difference-between-concurrency-and-parallelism).
