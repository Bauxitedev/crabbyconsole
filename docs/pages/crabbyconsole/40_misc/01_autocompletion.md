---
icon: lucide/message-square-code
---

# Autocompletion

When you start typing, autocompletion suggestions will show up below the text box. You can press ++tab++ to accept them, and press it again to go to the next suggestion. You can press ++shift+tab++ to go to the previous suggestion.

The autocompletion engine will do its best to try to figure out your intent, so it if you type this:
```
:load main
```

...and then press ++tab++, it will autocomplete to this[^1]: 
```
:load res://main.tscn
```
...so you don't have to type `res://` anymore. Neat!

[^1]: This assumes you have a scene called `main.tscn` in your game.