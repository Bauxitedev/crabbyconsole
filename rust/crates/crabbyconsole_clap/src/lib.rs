pub mod autocomplete;
pub mod clap_util;
pub mod cli;
pub mod command;
pub mod delta;
pub mod draw;
pub mod eval_definition;
pub mod plot;
pub mod smooth;
pub mod tween;
pub mod vr;

/*
Put all Clap structs here. Only types, no logic.
Do NOT move the ClapSubAction trait + its impls here, that stays in core.
That way, the clap derive macro stuff should not recompile when we only change evaluation logic, not the structure of the data types.
*/
