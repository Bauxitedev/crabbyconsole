use clap::Subcommand;
#[derive(Subcommand)]
pub enum CommandAction {
    /// Add a custom command
    Add {
        /// Name of the command
        name: String, // TODO use CommandName instead of String and parse it directly?

        /// Expression to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1.., required = true)]
        body: Vec<String>,
    },

    /// Remove a custom command
    #[command(alias = "rm")]
    Remove {
        /// Name of the command to remove
        name: String,
    },

    /// List all custom commands
    List,

    /// Clear all custom commands
    Clear,
}
