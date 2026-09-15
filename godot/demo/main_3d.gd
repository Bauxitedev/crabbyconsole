extends Node3D

func _ready() -> void:
	for i in 50:
		var ball = %Ball.duplicate()
		ball.name = "Ball%s" % i
		%Props.add_child(ball)
		
	print("\nHey you! Try pressing the 'Add example graphs' button in the bottom left to see some example graphs!")

func _on_commands_button_pressed() -> void:
	# TODO: eval these from a autoload, NOT the main scene, or it will re-run every time you switch scenes.
	# setup watches
	CrabbyConsole.eval_blocking(":watch add --rate 2 fps :perf fps")
	CrabbyConsole.eval_blocking(":watch add --rate 1 vsync :win vsync")
	CrabbyConsole.eval_blocking(":watch add --rate 1 log_level :log level")

	# setup key binds
	CrabbyConsole.eval_blocking(":key bind 0 :speed 1")
	CrabbyConsole.eval_blocking(":key bind 1 :speed 3")
	CrabbyConsole.eval_blocking(":key bind 2 :speed 9")
	CrabbyConsole.eval_blocking(":key bind 3 :speed 0.3")
	CrabbyConsole.eval_blocking(":key bind 4 :speed 0.1")
	CrabbyConsole.eval_blocking(":key bind X :cam proj toggle")

	histogram_off()
	
	CrabbyConsole.eval_blocking(":watch add --rate 2 ram :plot line -t 30 -y 0..1000 :perf mem")
	CrabbyConsole.eval_blocking(":watch add --rate 1 vram :plot line -t 20 -y 0..2000 Performance.get_monitor(Performance.RENDER_VIDEO_MEM_USED)/1024.0/1024.0")
	if false:
		CrabbyConsole.eval_blocking(":watch add --rate 4 read :plot line -t 5  :perf disk read")
		CrabbyConsole.eval_blocking(":watch add --rate 4 write :plot line -t 5  :perf disk write")
		
	# don't do this, it overrides the RUST_LOG env var you pass to `just test`
	# CrabbyConsole.eval_blocking(":log level info") 

	CrabbyConsole.add_custom_command("histogram-on", histogram_on)
	CrabbyConsole.add_custom_command("histogram-off", histogram_off)
	
	print("Example graphs added.")

func histogram_on():
	CrabbyConsole.eval_blocking(":watch add frametime :plot histo -m advanced :perf frame-time --ms")

func histogram_off():
	CrabbyConsole.eval_blocking(":watch add frametime :plot line -t 5 -y 0..100 -m advanced --threshold 16.666 --threshold 33.333 :perf frame-time --ms")
