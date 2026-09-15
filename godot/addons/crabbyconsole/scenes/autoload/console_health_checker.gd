extends Node

# Check if the console failed to load after 1 second.
# This must be in a separate script that does not derive from CrabConsole, or this script wouldn't even compile.
func _ready() -> void:
	await get_tree().create_timer(1.0).timeout
	
	if !ClassDB.class_exists("CrabConsole"):
		push_error("CrabbyConsole failed to load - ensure the library file (crabbyconsole.dll or libcrabbyconsole.so) is in your game's folder")
		%ConsoleCanvasLayer.hide()
		%PickerCanvasLayer.hide()
