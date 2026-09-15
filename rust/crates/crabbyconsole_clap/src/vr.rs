use clap::Subcommand;

use crate::clap_util::{BoolArg, positive_finite_num};

#[derive(Subcommand)]
pub enum VrAction {
    /// Enter VR
    Enter,

    /// Set VR console scale
    Scale {
        /// The scale of the VR console (0.5 = half as big, 2 = twice as big)
        #[arg(value_parser = positive_finite_num::<f32>)]
        scale: f32,
    },

    /// Enable/disable camera follow
    Follow {
        /// Whether the console should move with the camera or not
        follow: BoolArg,
    },
}
// TODO add command :cons vr res 1280 720?
