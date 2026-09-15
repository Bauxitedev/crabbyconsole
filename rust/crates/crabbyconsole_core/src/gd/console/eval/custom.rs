//! Custom command stuff

use std::{fmt, sync::Arc};

use clap::CommandFactory as _;
use color_eyre::eyre::{Report, eyre};
use crabbyconsole_clap::{command::CommandAction, eval_definition::MainCommand};
use crabbyconsole_misc::{gd::async_node::AsyncGd, reflection::OpaqueDebug};
use godot::{classes::Script, global::push_error, prelude::*};
use indexmap::IndexSet;

use crate::gd::console::{CrabConsole, eval::ClapSubAction};

impl CrabConsole {
    pub(crate) fn add_custom_command_inner(
        &mut self,
        name: String,
        func: Callable,
        help: GString,
    ) -> godot::global::Error {
        let add = || {
            self.ensure_ready("add_custom_command")?;

            let script = {
                let script_context = self.script_context.clone();
                script_context.get_script() // get_script() can be None
                // TODO - i think we not only need to keep the script around, but also the object it's attached to
                // the closures are still getting destroyed!
            };

            // If empty, the value was likely not passed in, so generate one
            let help = if help.is_empty() {
                callable_to_help(&func)
            } else {
                help.into()
            };

            self.commands.insert(
                CommandName::new(name)?, // <-- now also checks if the name is reserved, not only valid
                CustomCommand {
                    expression: CustomCommandExpression::Callable(OpaqueDebug(func)),
                    help,
                    script,
                    // We need to keep the ScriptContext's script around,
                    // otherwise if you call add_custom_command in the CrabConsole with a closure, the closure gets destroyed immediately
                    // TODO that doesn't actually work: you need to store the object the script belongs to as well.
                },
            );

            Ok::<_, Report>(())
        };

        match add() {
            Ok(()) => godot::global::Error::OK,
            Err(err) => {
                tracing::warn!(%err, "failed to add custom command");
                push_error(&[Variant::from(format!(
                    "failed to add custom command: {err}"
                ))]);

                // TODO use error_string to make this human-readable?
                godot::global::Error::ERR_INVALID_PARAMETER
            }
        }
    }
}

/// Newtype around String to ensure command name is valid and doesn't mess up clap's parser
/// also checks if the name is not reserved by a built-in command.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CommandName(String);

impl CommandName {
    pub fn new(name: impl Into<String>) -> Result<Self, Report> {
        let name = name.into();

        if !Self::is_valid(&name) {
            return Err(eyre!(
                "'{name}' is not a valid command name (it must be alphanumeric, must not start with a dash, and cannot be `help`)"
            ));
        }

        if !Self::is_available(&name) {
            return Err(eyre!(
                "'{name}' is reserved (a custom command cannot have the same name as a built-in command)"
            ));
        }

        Ok(Self(name))
    }

    fn is_valid(name: &str) -> bool {
        !name.is_empty()
            && !name.starts_with('-')
            && name != "help" // Important check, otherwise users can override `help` (it does NOT show up in get_subcommands(), so it will NOT be reserved)
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }

    fn is_available(name: &str) -> bool {
        // Reserve all subcommands + all their aliases, to prevent users overwriting them and causing mayhem
        // Note - `help` is NOT reserved, so is_valid() catches it instead.

        let cmd = MainCommand::command();
        let reserved = cmd
            .get_subcommands()
            .flat_map(|cmd| {
                vec![cmd.get_name()]
                    .into_iter()
                    .chain(cmd.get_all_aliases())
            })
            .collect::<IndexSet<_>>();

        !reserved.contains(name)
    }
}

impl fmt::Display for CommandName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::borrow::Borrow<str> for CommandName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)] // Cloning a Callable seems to create another one that refers to the same function
pub(crate) enum CustomCommandExpression {
    String(Arc<str>),
    Callable(OpaqueDebug<Callable>),
    // Important: do not remove OpaqueDebug here! Otherwise there is potential for a crash in :debug print:
    // 1. Run CrabbyConsole.add_custom_command("crash", func(): print("hi"))
    // 2. Run :crash
    // 3. Run :debug print
    // Boom, crash!
}

#[derive(Debug, Clone)]
pub(crate) struct CustomCommand {
    pub(crate) expression: CustomCommandExpression,
    pub(crate) help: String,
    pub(crate) script: Option<Gd<Script>>, // <-- Only used to keep closures alive (TODO does not actually keep them alive - need to store the Gd<Object> the script belongs to as well)
}

impl ClapSubAction for CommandAction {
    async fn handle(self, mut console: AsyncGd<CrabConsole>) -> Result<Variant, Report> {
        {
            match self {
                CommandAction::Add { name, body } => {
                    let name = CommandName::new(name)?;

                    let expression: Arc<str> = Arc::from(body.join(" "));

                    console.bind_mut().commands.insert(
                        name,
                        CustomCommand {
                            expression: CustomCommandExpression::String(Arc::clone(&expression)),
                            help: expression.to_string(), // Clones the String
                            script: None,
                        },
                    );
                    Ok(Variant::nil())
                }
                CommandAction::Remove { name } => {
                    console.bind_mut().commands.shift_remove(name.as_str());
                    Ok(Variant::nil())
                }
                CommandAction::List => {
                    let list = console
                        .bind()
                        .commands
                        .iter()
                        .map(|(name, CustomCommand { expression, .. })| {
                            format!("{name} = {expression:?}")
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(Variant::from(list))
                }
                CommandAction::Clear => {
                    console.bind_mut().commands.clear();
                    Ok(Variant::nil())
                }
            }
        }
    }
}

/////////////

fn callable_to_help(func: &Callable) -> String {
    // Important: check if valid first, or you can get a segfault here
    if func.is_valid() {
        format!(
            "Custom command (needs {}+ arguments) => {:?}.{:?}",
            func.get_argument_count(),
            func.object(),
            func.method_name() // if invalid, it segfaults here in:
                               // Callable::get_method -> GDscriptLambdaCallable::get_method -> GDScriptFunction::get_name -> StringName copy constructor
        )
    } else {
        "<invalid callable>".into()
    }
}
