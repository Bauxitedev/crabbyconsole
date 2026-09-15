---
icon: lucide/gamepad-2
---

# Platform support

## Supported platforms

| Platform  | Supported |
| ------------- | ------------- |
| Windows  | :lucide-check:  |
| Linux | :lucide-check: |
| MacOS |  :lucide-ban: [^1]  |
| Web | :lucide-ban: [^1] |
| Android | :lucide-ban: [^1] |
| iOS | :lucide-ban: [^4] |

[^1]: Not supported yet, contributions welcome!
[^4]: CrabbyConsole depends on `godot-rust`, [which does not support iOS yet](https://github.com/godot-rust/gdext/issues/498).


## Supported Godot versions

| Godot version  | Supported  |
| ------- | ----------------- |
| ≤4.4.x  | :lucide-ban: [^2] |
| 4.5.x   | :lucide-ban: [^3] |
| 4.6.x   |  :lucide-check:   |
| 4.7.x   |   :lucide-check:  |
| 4.8.x   |   :lucide-check:  |

[^2]: Not supported due to the [Logger class](https://docs.godotengine.org/en/stable/classes/class_logger.html) not being available.
[^3]: Support may be added if there is enough demand.
