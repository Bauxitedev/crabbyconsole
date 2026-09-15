@tool
extends EditorPlugin

# Add autoloads here.
func _enable_plugin() -> void:
	print("CrabbyConsole _enable_plugin")
	add_autoload_singleton("CrabbyConsole", "res://addons/crabbyconsole/scenes/autoload/console.tscn")

# Remove autoloads here.
func _disable_plugin() -> void:
	print("CrabbyConsole _disable_plugin")
	remove_autoload_singleton("CrabbyConsole")

# Initialization of the plugin goes here.
func _enter_tree() -> void:
	pass
	#print("CrabbyConsole _enter_tree")
	
# Cleanup of the plugin goes here.
func _exit_tree() -> void:
	pass
	#print("CrabbyConsole _exit_tree")
