use clap::Args;

#[derive(Args)]
pub struct DeltaArgs {
    /// The expression to differentiate. Must be numerical, `Vector`-like, or `Quat`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
    pub expression: Vec<String>,

    /// If set, calculate the norm (aka magnitude) of the delta - useful to determine the (angular) speed of an object in any dimension
    #[arg(short, long)]
    pub norm: bool,

    /// If set, divides the result by `dt` (the amount of time since the previous result was calculated). In other words, it will calculate the delta *per second* instead of *per frame*
    #[arg(long)]
    pub dt: bool,
}
