---
icon: lucide/sparkle
---

# Advanced commands

Here are some more advanced examples of what you can do.

### Create a 4x4x4 grid of spheres (in 3D)
```gdscript
:for x 1 4 :for y 1 4 :for z 1 4 var s = MeshInstance3D.new(); s.mesh = SphereMesh.new(); add_child(s); s.global_position = Vector3({x},{y},{z})
```

### Play a beep sound every time you resize the game window
```
:watch add annoying :seq viewport().size_changed |> :beep
```

### Play a beep sound when you type a number
```
:watch add number_pressed :seq :select i i.is_valid_int() =-> :key await |> :beep
```
Note: this only works if you type something outside the text box.
