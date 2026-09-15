#!/usr/bin/env -S godot -s
extends SceneTree

func _init():
    # wait 1 frame, otherwise there are no autoloads yet
    await process_frame
  
    # TODO why pass the args here? why can't `run_from_args` just get the args itself?
    var exit_code = CrabConsoleTestRunner.run_from_args(OS.get_cmdline_user_args())
    quit(exit_code)