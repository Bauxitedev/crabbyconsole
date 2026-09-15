---
icon: lucide/gauge
---

# Performance

Adding CrabbyConsole to your game shouldn't affect performance a lot. If it does, that's a bug, please open an issue.

If you have a lot of watches, it's better to combine them into one big watch.
E.g. instead of doing:
```
:watch add a 1
:watch add b 2
:watch add c 3
```
...it's better to do this:

```
:watch add abc [1, 2, 3]
```
Since this requires evaluating only one expression instead of three.

Also, put a rate limit on it, so it only evaluates at most once per second, instead of every frame:
```
:watch add --rate 1 abc [1, 2, 3]
```

Note that watches keep evaluating in the background even when CrabbyConsole is hidden, so be sure to remove all watches when you don't need them anymore:
```
:watch clear
```