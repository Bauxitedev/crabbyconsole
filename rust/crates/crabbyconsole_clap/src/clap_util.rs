use std::{fmt::Display, ops::Range, str::FromStr};

use clap::{ValueEnum, builder::PossibleValue};
use godot::prelude::*;
use ordered_float::NotNan;
use tap::Tap as _;

/// Put your marker traits here
pub mod marker {

    use clap::builder::CommandExt;

    /// Marker struct to mark a subcommand (and all of its subcommands) as risky.
    /// Risky commands cannot be executed in lockdown mode, and they will show a warning on their documentation page.
    #[derive(Debug, Clone)]
    pub struct Risky;

    /// Marker struct to mark a subcommand (and all of its subcommands) as experimental.
    /// Experimental commands will show a warning on their documentation page.
    #[derive(Debug, Clone)]
    pub struct Experimental;

    impl CommandExt for Risky {}
    impl CommandExt for Experimental {}
}

/// Little helper so we can do stuff like `flag 1/0/true/false/on/off/toggle`.
/// See <https://github.com/clap-rs/clap/issues/1649>
#[derive(Clone)]
pub enum BoolArg {
    Set(bool),
    Toggle,
}

impl ValueEnum for BoolArg {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Set(true), Self::Set(false), Self::Toggle]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            Self::Set(true) => PossibleValue::new("1").alias("true").alias("on"),
            Self::Set(false) => PossibleValue::new("0").alias("false").alias("off"),
            Self::Toggle => PossibleValue::new("toggle"),
        })
    }
}

pub const POSITION_ARG_HELP: &str = "x,y | x,y,z | @cursor | @cam | @<NODE>"; // <NODE> means a node pattern like Static*3D

#[derive(Debug, Clone)]
pub enum PositionArg {
    Coords2D(Vector2),
    Coords3D(Vector3),
    Node(String), // Pattern -> note that . : @ / " % are not valid in Godot node names, so we can use them to add extra functionality (e.g. Ship:rot to also set the rotation, not just the position, to be a copy of Ship's)
    Cursor, // TODO in ^^^ we could filter on node type, e.g. if user requested 2d position, find a Node2d, else a Node3d
    Camera,
    // TODO add Variable(string)?
    //Player,
    //Origin,
}

impl FromStr for PositionArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(pattern) = s.strip_prefix('@') {
            return match pattern {
                "cursor" => Ok(PositionArg::Cursor),
                "cam" => Ok(PositionArg::Camera),
                _ if !pattern.is_empty() => Ok(PositionArg::Node(pattern.to_string()))
                    .tap(|parsed| tracing::warn!(pattern, ?parsed, "parsed position as Node:")),
                _ => Err("node pattern cannot be empty".to_string()),
            };
        }

        parse_vec3(s)
            .map(PositionArg::Coords3D)
            .or_else(|_err| parse_vec2(s).map(PositionArg::Coords2D)) // TODO if that fails, try to parse variable name?
    }
}

pub fn parse_vec2(s: &str) -> Result<Vector2, String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return Err(format!("expected 'x,y' but got '{}'", s));
    }
    let x = parts[0]
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("invalid x: '{}'", parts[0]))?;
    let y = parts[1]
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("invalid y: '{}'", parts[1]))?;

    Ok(Vector2::new(x, y))
}

pub fn parse_vec3(s: &str) -> Result<Vector3, String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(format!("expected 'x,y,z' but got '{}'", s));
    }
    let x = parts[0]
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("invalid x: '{}'", parts[0]))?;
    let y = parts[1]
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("invalid y: '{}'", parts[1]))?;
    let z = parts[2]
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("invalid z: '{}'", parts[2]))?;
    Ok(Vector3::new(x, y, z))
}

/// Use this in Clap like `#[arg(value_parser = non_negative_finite_num::<f32>)]` to
/// ensure it's a positive finite number (float/integer both work).
/// Zero is allowed here.
pub fn non_negative_finite_num<T>(s: &str) -> Result<T, String>
where
    T: FromStr + FiniteCheck + Display,
    T::Err: Display,
{
    let v: T = s
        .parse()
        .map_err(|e| format!("'{}' is not a valid number: {}", s, e))?;
    v.check_finite()?;
    v.check_non_negative()?;
    Ok(v)
}

/// Use this in Clap like `#[arg(value_parser = positive_finite_num::<f32>)]` to
/// ensure it's a positive finite number (float/integer both work).
/// Note, the number must be > 0, so 0 itself is not allowed.
pub fn positive_finite_num<T>(s: &str) -> Result<T, String>
where
    T: FromStr + FiniteCheck + Display,
    T::Err: Display,
{
    let v: T = s
        .parse()
        .map_err(|e| format!("'{}' is not a valid number: {}", s, e))?;
    v.check_finite()?;
    v.check_positive()?;
    Ok(v)
}

/// Similar to `positive_finite_num`, except also allows negative numbers.
/// 0 is still disallowed.
pub fn nonzero_finite_num<T>(s: &str) -> Result<T, String>
where
    T: FromStr + FiniteCheck + Display,
    T::Err: Display,
{
    let v: T = s
        .parse()
        .map_err(|e| format!("'{}' is not a valid number: {}", s, e))?;
    v.check_finite()?;
    v.check_nonzero()?;
    Ok(v)
}

pub fn validate_no_braces(s: &str) -> Result<String, String> {
    if s.contains('{') || s.contains('}') {
        Err(format!("value must not contain '{{' or '}}': `{s}`"))
    } else {
        Ok(s.to_string())
    }
}

///////// RANGE PARSER /////////

/// Little helper trait so `parse_range::<T>` can reject T if it's inf/NaN.
/// Also used in `positive_finite_num`
pub trait FiniteCheck: PartialOrd + Default {
    fn check_finite(&self) -> Result<(), String>;
    fn check_positive(&self) -> Result<(), String> {
        if *self <= Self::default() {
            Err("value must be positive".to_string())
        } else {
            Ok(())
        }
    }
    fn check_non_negative(&self) -> Result<(), String> {
        if *self < Self::default() {
            Err("value must be non-negative".to_string())
        } else {
            Ok(())
        }
    }
    fn check_nonzero(&self) -> Result<(), String> {
        if *self == Self::default() {
            Err("value must not be zero".to_string())
        } else {
            Ok(())
        }
    }
}

// I removed the blanked impl for FiniteCheck:
// impl<T: PartialOrd + Default> FiniteCheck for T { ... }
// ...because it pretends any unknown type is always finite.
// This is definitely not the case for things that wrap f32, like NotNan<f32> or OrderedFloat<f32>.
// It would silently do the wrong thing in the clap parser, since NotNan<f32>::from(f32::inf) would wrongly claim it's NOT infinite.
// So now we manually have to handle every type, instead of just blanked impl accepting anything as finite by default.

// i64 is always finite
impl FiniteCheck for i64 {
    fn check_finite(&self) -> Result<(), String> {
        Ok(())
    }
}
impl FiniteCheck for f32 {
    fn check_finite(&self) -> Result<(), String> {
        if self.is_finite() {
            Ok(())
        } else {
            Err("f32 must be finite (not NaN or infinite)".to_string())
        }
    }
}

impl FiniteCheck for NotNan<f32> {
    fn check_finite(&self) -> Result<(), String> {
        self.into_inner().check_finite() // delegate to f32::FiniteCheck
    }
}

impl FiniteCheck for f64 {
    fn check_finite(&self) -> Result<(), String> {
        if self.is_finite() {
            Ok(())
        } else {
            Err("f64 must be finite (not NaN or infinite)".to_string())
        }
    }
}

impl FiniteCheck for NotNan<f64> {
    fn check_finite(&self) -> Result<(), String> {
        self.into_inner().check_finite() // delegate to f64::FiniteCheck
    }
}

/// Parse a range like 5..10 or -4.2..9.9. Rejects inf/NaN, so 0..inf is not allowed.
/// Works for any T that implements `PartialOrd`, so integers and characters should work as well.
/// Note - unbounded ranges like 0.. are not allowed at the moment, would be useful for `:plot line` though.
pub fn parse_range<T>(s: &str) -> Result<Range<T>, String>
where
    T: FromStr + PartialOrd + Display + FiniteCheck,
    T::Err: Display,
{
    let (start_s, end_s) = s
        .split_once("..")
        .ok_or_else(|| format!("invalid range `{s}`, expected format START..END"))?;

    let start: T = start_s
        .parse()
        .map_err(|e| format!("invalid start `{start_s}`: {e}"))?;
    let end: T = end_s
        .parse()
        .map_err(|e| format!("invalid end `{end_s}`: {e}"))?;

    // These checks are only implemented for f32/f64 for now.
    start.check_finite().map_err(|e| format!("start: {e}"))?;
    end.check_finite().map_err(|e| format!("end: {e}"))?;

    if start >= end {
        return Err(format!(
            "range start ({start}) must be less than end ({end})"
        ));
    }

    Ok(start..end)
}
