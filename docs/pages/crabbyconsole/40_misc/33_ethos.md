---
icon: lucide/brain
---

# Ethos

The ethos of this project is to:

1. Let you do as many things as possible without needing to restart the game. It should be possible to brainstorm new gameplay ideas and mechanics in real time, because... restarting the game means:
    - You lose your current state, making it harder to get back to that specific state that may cause a hard-to-reproduce bug.
    - If you have a large game, it may take a long time to load the game back up.
2. Be self-contained, so it should not depend on the Godot editor, and function in exported games as well. Why? 
    - Bugs may show up only in the exported game, making them impossible to debug while running your game in the editor.
    - Performance metrics may change drastically in an exported build.
    - It allows developers to use this as a "tech support" tool, e.g. users can screenshot performance metrics for developers to diagnose problems that only occur on their user's devices. (No more *"works on my machine"*.)
3. Be as transparent as possible. Every aspect of your game should be inspectable and visualizable, even the console itself. There shall be no more *"I have no idea what is happening in the background"*. And no more "print statement debugging" either: using watches, you can see the value of any variable in real time, and you can also visually see signals being emitted in real time (see section [Watching expressions](../20_commands/04_watches.md)).

