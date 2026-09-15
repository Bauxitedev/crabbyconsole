---
icon: lucide/paintbrush
---

# Customization

To customize the console, e.g. for aesthetic reasons, or in case you want to draw something on top of the console, you can patch the console by adjusting its parameters. E.g. to lower the canvas layer index of the console, so you can draw things on top of it, you can run this somewhere in your game at startup:

```gdscript title="GDScript"
CrabbyConsole.get_node("%ConsoleCanvasLayer").layer = 100
```

Or to make it more transparent:

```gdscript
CrabbyConsole.get_node("%ConsolePanel").modulate.a = 0.5
```

This is more robust than editing the `console.tscn` file manually, because it will keep working even when an update for CrabbyConsole comes out.

