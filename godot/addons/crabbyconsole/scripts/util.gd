class_name CrabConsoleUtil extends Node

static func lerp_smooth_float(a: Variant, b: Variant, lerp_speed: float, delta: float) -> Variant:
	var t = clamp(1.0 - exp(-delta * lerp_speed), 0.0, 1.0)
	return lerp(a, b, t)
	
static func lerp_smooth(a: Variant, b: Variant, lerp_speed: float, delta: float) -> Variant:
	var t = clamp(1.0 - exp(-delta * lerp_speed), 0.0, 1.0)
	return a.lerp(b, t)
