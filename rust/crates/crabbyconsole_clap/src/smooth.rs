use clap::Args;

use crate::clap_util::positive_finite_num;

#[derive(Args)]
pub struct SmoothArgs {
    /// The speed of the smoothness (higher = less smooth)
    #[arg(short, long, value_parser = positive_finite_num::<f64>, default_value_t = 5.)]
    pub speed: f64,

    /// The expression to smooth. Must be numerical
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
    pub expression: Vec<String>,
}
