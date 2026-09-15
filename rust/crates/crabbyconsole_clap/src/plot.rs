use std::ops::Range;

use clap::{Subcommand, ValueEnum};
use ordered_float::NotNan;

use crate::clap_util::{parse_range, positive_finite_num};
#[derive(Subcommand, Clone, Hash, PartialEq, Eq)]
pub enum PlotAction {
    /// Draw a line plot
    Line {
        /// The time range of the plot, in seconds
        #[arg(short, long, default_value_t = NotNan::from(4_i16), value_parser = positive_finite_num::<NotNan<f32>>)]
        time: NotNan<f32>,

        /// The vertical range of the plot, e.g. -5..10
        #[arg(short, long, value_parser = parse_range::<NotNan<f32>>)]
        y_range: Option<Range<NotNan<f32>>>,

        /// Draw a red line at the given y coordinate (useful to show critical thresholds on the graph). Can be specified multiple times to draw multiple lines
        #[arg(long)]
        threshold: Vec<NotNan<f32>>,

        /// What kind of metrics to draw on the plot
        #[arg(short, long, value_enum, default_value_t = MetricsKind::Basic)]
        metrics: MetricsKind,

        /// The maximum rate at which to update the plot texture, in Hz (note - values are always gathered, but the plot texture itself is rate limited for performance reasons)
        #[arg(short, long, default_value_t = NotNan::from(15_i16), value_parser = positive_finite_num::<NotNan<f32>>)]
        rate: NotNan<f32>,

        /// The expression to plot. Should return a numerical value such as float or int.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        expression: Vec<String>,
    },

    /// Draw a histogram
    Histo {
        /// The horizontal range of the plot, e.g. -5..10
        #[arg(short, long, value_parser = parse_range::<NotNan<f32>>)]
        x_range: Option<Range<NotNan<f32>>>,

        /// The vertical range of the plot, e.g. 0..100
        #[arg(short, long, value_parser = parse_range::<NotNan<f32>>)]
        y_range: Option<Range<NotNan<f32>>>,

        /// Draw a red line at the given x coordinate (useful to show critical thresholds on the histogram). Can be specified multiple times to draw multiple lines
        #[arg(long)]
        threshold: Vec<NotNan<f32>>,

        /// What kind of metrics to draw on the plot
        #[arg(short, long, value_enum, default_value_t = MetricsKind::Basic)]
        metrics: MetricsKind,

        /// The maximum rate at which to update the plot texture, in Hz (note - values are always gathered, but the plot texture itself is rate limited for performance reasons)
        #[arg(short, long, default_value_t = NotNan::from(15_i16), value_parser = positive_finite_num::<NotNan<f32>>)]
        rate: NotNan<f32>,

        /// The expression to plot. Should return a numerical value such as float or int.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        expression: Vec<String>,
    },
    // TODO: how to add --width and --height across ALL enums?
    // There may be a trick so you don't have to add it to all variants...
    // Maybe you can use #[flatten] and also maybe groups: https://docs.rs/clap/latest/clap/struct.ArgGroup.html:
    // I recommend putting all 6 fields in a flattened struct (x_range, y_range, threshold, metrics, rate, expression), but retaining Line.time
    // Then, using a clap Group, you can check that `x_range` and `time` are mutually exclusive
    // (so you can't set both simultaneously)
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq, Hash)]
pub enum MetricsKind {
    None,
    Basic,
    Advanced,
}
