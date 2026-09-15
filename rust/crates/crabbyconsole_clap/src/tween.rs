use clap::Subcommand;

use crate::clap_util::{POSITION_ARG_HELP, PositionArg};

#[derive(Subcommand)]
pub enum TweenAction {
    /// Tween 3D position of a node (or nodes)
    #[command(visible_aliases = ["pos"])]
    Position {
        /// Node to tween
        node: String,

        /// Position to tween from
        #[arg(short, long, value_name = POSITION_ARG_HELP)]
        from: Option<PositionArg>, // TODO support 2d position as well

        /// Position to tween to
        #[arg(value_name = POSITION_ARG_HELP)]
        to: PositionArg, // TODO support 2d position as well

        /// Tween duration (in seconds)
        duration: f64,

        // Tween configuration
        // config: TweenConfig, //TODO allow presets like smooth/linear/elastic?
        /// Tween *all* nodes that match the given pattern, instead of just the first one
        #[arg(short, long)]
        all: bool,
    },

    /// Tween any property of a node (or nodes)
    #[command(visible_aliases = ["prop"], allow_negative_numbers = true)]
    Property {
        /// Node to tween
        node: String,

        /// Property to tween
        prop: String,

        /// Value to tween from
        #[arg(short, long)]
        from: Option<String>,

        /// Value to tween to
        to: String,

        /// Tween duration (in seconds)
        duration: f64,

        // Tween configuration
        // config: TweenConfig, //TODO allow presets like smooth/linear/elastic?
        /// Tween *all* nodes that match the given pattern, instead of just the first one
        #[arg(short, long)]
        all: bool,
    },
    // TODO add Callback maybe? But then we need Func first otherwise we can't define a function
}
