shader_type canvas_item;

uniform bool enabled = true;
uniform float speed : hint_range(0.0, 100.0) = 1.0;
uniform float wavelength : hint_range(1.0, 500.0) = 10.0; // in pixels

void fragment() {
	vec4 tex_color = texture(TEXTURE, UV);

	if (enabled) {
		float wave = sin((FRAGCOORD.x / wavelength) * TAU - TIME * speed) * 0.4 + 0.6;
		tex_color.a *= wave;
	}

	COLOR = tex_color;
}