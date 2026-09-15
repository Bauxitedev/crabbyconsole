extends Node2D

var text: String: get = get_text, set = set_text
var slot_text: String: get = get_slot_text, set = set_slot_text
@onready var label = %PickerLabel
@onready var slot_label = %PickerSlotLabel
@onready var panel: MarginContainer = %PickerPanel
@onready var panel_visible: get = get_panel_visible, set = set_panel_visible

var target_global_position = Vector2()
var target_size = Vector2()

func _process(delta: float) -> void:
	if panel_visible:
		panel.modulate = Color(1., 1., 1., lerp(0.4, 0.8, fposmod(Time.get_ticks_msec() / 1000.0 * -1.5, 1.0)))
		
		# animate panel
		panel.global_position = CrabConsoleUtil.lerp_smooth(panel.global_position, target_global_position, 30.0, delta)
		panel.size = CrabConsoleUtil.lerp_smooth(panel.size, target_size, 30.0, delta)

###

func get_text() -> String:
	return label.text

func set_text(value: String):
	label.text = value
	
###
	
func get_slot_text() -> String:
	return slot_label.text

func set_slot_text(value: String):
	slot_label.text = value
	
###

func set_panel_rect_global(rect: Rect2):
	target_global_position = rect.position
	target_size = rect.size

func get_panel_visible() -> bool:
	return panel.visible
	
func set_panel_visible(value: bool):
	panel.visible = value
