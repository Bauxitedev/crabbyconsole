---
icon: lucide/logs
---

# Log hook

By default, the console will register a [Logger](https://docs.godotengine.org/en/stable/classes/class_logger.html) to hook Godot's output and print it the console. This is useful to see your game's stdout and stderr, without having to run your game from the terminal.

If you want to disable this, you can run:
```
:log hook 0
```