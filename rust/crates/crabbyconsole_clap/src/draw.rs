use clap::{Args, Subcommand, ValueEnum};
use color_eyre::eyre::{Report, eyre};
use godot::prelude::*;

use crate::clap_util::{non_negative_finite_num, positive_finite_num};
#[derive(Subcommand)]
pub enum DrawAction {
    /// Draw a 2D shape
    #[command(name = "2d", subcommand)]
    TwoD(DrawAction2D),

    /// Draw a 3D shape
    #[command(name = "3d", subcommand)]
    ThreeD(DrawAction3D),
}

#[derive(Subcommand)]
pub enum DrawAction2D {
    /// Draw a line (or multiple lines)
    Line(LineArgs),

    /// Draw a rectangle
    Rect(HyperrectangleArgs),

    /// Draw a circle
    Circle(HypersphereArgs),

    /// Draw text
    Text {
        /// The lifetime of the text, in seconds (0 = only drawn for one frame)
        #[arg(short, long, default_value_t = 1., value_parser = non_negative_finite_num::<f32>)]
        time: f32,

        /// Color of the text
        #[arg(short, long, default_value = "green", value_parser = color_parser)]
        color: Color,

        /// Horizontal alignment of the text
        #[arg(short, long, value_enum, default_value_t = TextAlignment::Left)]
        align: TextAlignment,

        /// Position and text in the form of an expression that returns an array `[pos, text]`, e.g. `[v2(0, 0), "hi"]`. `text` can also be a Variant, in which case it will be stringified. If it's an array, every entry in the array will be drawn on a separate line.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        pos_text: Vec<String>,
    },

    /// Clear all 2D shapes
    Clear,
}

#[derive(Subcommand)]
pub enum DrawAction3D {
    /// Draw a line (or multiple lines)
    Line(LineArgs),

    /// Draw a AABB (aka a cuboid)
    Aabb(HyperrectangleArgs),

    /// Draw a sphere
    Sphere(HypersphereArgs),

    /// Draw a `Transform3D` as three colored arrows at the transform's origin
    Transform {
        /// The lifetime of the `Transform3D`, in seconds (0 = only drawn for one frame)
        #[arg(short, long, default_value_t = 1., value_parser = non_negative_finite_num::<f32>)]
        time: f32,

        /// Scale of the basis arrows to draw
        #[arg(short, long, default_value_t = 1., value_parser = positive_finite_num::<f32>)]
        scale: f32,

        /// Transform to draw, in the form of an expression that returns a `Transform3D`, e.g. `Transform3D.IDENTITY`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        transform3d: Vec<String>,
    },

    /// Clear all 3D shapes
    Clear,
}
#[derive(Args)]
pub struct LineArgs {
    /// The lifetime of the line, in seconds (0 = only drawn for one frame)
    #[arg(short, long, default_value_t = 1., value_parser = non_negative_finite_num::<f32>)]
    pub time: f32,

    /// Color of the line
    #[arg(short, long, default_value = "green", value_parser = color_parser)]
    pub color: Color,

    /// Expression that returns an array `[from, to]`, where `from` and `to` are both `Vector2` or `Vector3` (or a `Node`). The array can also be longer to draw a polyline
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
    pub from_to: Vec<String>,
}

/// Protip: a "hyperrectangle" is the generalization of a rectangle to any dimension
#[derive(Args)]
pub struct HyperrectangleArgs {
    /// The lifetime of the rectangle/box, in seconds (0 = only drawn for one frame)
    #[arg(short, long, default_value_t = 1., value_parser = non_negative_finite_num::<f32>)]
    pub time: f32,

    /// Color of the rectangle/box
    #[arg(short, long, default_value = "green", value_parser = color_parser)]
    pub color: Color,

    /// Bounds of the rectangle/box to draw, in the form of an expression that returns a `Rect2` or `AABB` or `Node`, e.g. `r2(v2(100, 200), v2(500, 600))`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
    pub rect: Vec<String>,
}

/// Protip: a "hypersphere" is the generalization of a sphere to any dimension
#[derive(Args)]
pub struct HypersphereArgs {
    /// The lifetime of the sphere, in seconds (0 = only drawn for one frame)
    #[arg(short, long, default_value_t = 1., value_parser = non_negative_finite_num::<f32>)]
    pub time: f32,

    /// Color of the sphere
    #[arg(short, long, default_value = "green", value_parser = color_parser)]
    pub color: Color,

    /// Radius of the sphere
    #[arg(short, long, default_value_t = 10., value_parser = positive_finite_num::<f32>)]
    pub radius: f32, // Default radius = 10 -> good for 2D, but a little too big for 3D,

    /// Position of the sphere, in the form of an expression that returns `Vector2` or `Vector3` or `Node`, e.g. `v2(100, 200)`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
    pub pos: Vec<String>,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum TextAlignment {
    Left,
    Center,
    Right,
}

fn color_parser(color: &str) -> Result<Color, Report> {
    let color = Color::from_string(color).ok_or_else(|| {
        eyre!(
            "invalid color `{color}`,\
                try passing a valid one like `red`, `cornflower_blue`, or `#FF00FF`"
        )
    })?;

    Ok(color)
}
