use std::{fs, io, path::Path};

use clap::{Arg, Command, CommandFactory as _};
use crabbyconsole_clap::{
    clap_util::marker::{Experimental, Risky},
    eval_definition::MainCommand,
};

fn main() -> std::io::Result<()> {
    let mut cmd = MainCommand::command();
    write_help_docs(
        &mut cmd,
        Path::new("../docs/pages/api_reference/commands/"),
        "",
    )?;
    Ok(())
}
const ABOUT_ONLY_TEMPLATE: &str = "{about}";
const USAGE_ONLY_TEMPLATE: &str = "{usage}";

/// Recursively writes the `--help` output of `cmd` and all of its subcommands
/// to markdown files, mirroring the subcommand tree as nested folders.
pub fn write_help_docs(cmd: &mut Command, out_dir: &Path, prefix: &str) -> io::Result<()> {
    write_help_docs_inner(cmd, out_dir, prefix, false, false)
}

fn write_help_docs_inner(
    cmd: &mut Command,
    out_dir: &Path,
    prefix: &str,
    inherited_risky: bool,
    inherited_experimental: bool,
) -> io::Result<()> {
    fs::create_dir_all(out_dir)?;

    // TODO the riskiness propagation logic is spread across two files.
    // Can we make it one source of truth?
    let is_risky = inherited_risky || cmd.get::<Risky>().is_some();
    let is_experimental = inherited_experimental || cmd.get::<Experimental>().is_some();

    let full_name = if prefix.is_empty() {
        cmd.get_name().to_string()
    } else {
        format!("{prefix} {}", cmd.get_name())
    };

    let about_text = render_with_template(cmd, ABOUT_ONLY_TEMPLATE, None);
    let usage_text = render_with_template(cmd, USAGE_ONLY_TEMPLATE, Some(&full_name));

    let mut markdown = format!("# `{full_name}`\n\n");

    if !about_text.is_empty() {
        markdown.push_str(&about_text);
        markdown.push_str("\n\n");
    }

    if !usage_text.is_empty() {
        markdown.push_str(&format!("Usage: `{usage_text}`\n\n"));
    }

    let arg_table = markdown_table(cmd.get_positionals().map(format_arg));
    if !arg_table.is_empty() {
        markdown.push_str("## Arguments\n\n");
        markdown.push_str(&arg_table);
        markdown.push('\n');
    }

    let opt_table = markdown_table(cmd.get_opts().map(format_arg));
    if !opt_table.is_empty() {
        markdown.push_str("## Options\n\n");
        markdown.push_str(&opt_table);
        markdown.push('\n');
    }

    if is_risky {
        markdown.push_str("!!! danger \"Risky\"\n");
        markdown.push_str(
            "    This command is marked as risky, so it cannot be used in lockdown mode.\n",
        );
    }

    if is_experimental {
        markdown.push_str("!!! example \"Experimental\"\n");
        markdown.push_str(
            "    This command is marked as experimental, so it may be subject to change.\n",
        );
    }

    fs::write(out_dir.join("index.md"), markdown)?;

    for sub in cmd.get_subcommands_mut() {
        let sub_dir = out_dir.join(sub.get_name());
        write_help_docs_inner(sub, &sub_dir, &full_name, is_risky, is_experimental)?;
    }

    Ok(())
}

fn render_with_template(cmd: &Command, template: &'static str, bin_name: Option<&str>) -> String {
    let mut c = cmd.clone().help_template(template);
    if let Some(bin_name) = bin_name {
        c = c.bin_name(bin_name.to_string());
    }
    c.render_long_help().to_string().trim().to_string()
}

struct ArgRow {
    name: String,
    help: String,
    possible_values: Vec<String>,
    default_values: Vec<String>,
}

fn format_arg(arg: &Arg) -> ArgRow {
    let name = if arg.is_positional() {
        if let Some(names) = arg.get_value_names() {
            names
                .iter()
                .map(|n| format!("`<{n}>`"))
                .collect::<Vec<_>>()
                .join("<br>")
        } else {
            format!("`<{}>`", arg.get_id().as_str().to_uppercase())
        }
    } else {
        let mut parts = Vec::new();
        if let Some(short) = arg.get_short() {
            parts.push(format!("`-{short}`"));
        }
        if let Some(long) = arg.get_long() {
            parts.push(format!("`--{long}`"));
        }
        parts.join("<br>")
    };

    let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();

    let default_values: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().to_string())
        .collect();

    // TODO this does not work properly for boolean properties like --verbose.
    // It shows "possible values: [true, false]" but it should just show "present" or "not present"
    let possible_values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|pv| !pv.is_hide_set())
        .map(|pv| pv.get_name().to_string())
        .collect();

    ArgRow {
        name,
        help,
        possible_values,
        default_values,
    }
}

fn markdown_table(items: impl Iterator<Item = ArgRow>) -> String {
    let rows: Vec<ArgRow> = items.filter(|row| !row.name.is_empty()).collect();

    if rows.is_empty() {
        return String::new();
    }

    let mut table = String::from(
        "| Name | Description | Possible values | Default |\n| --- | --- | --- | --- |\n",
    );
    for row in rows {
        let help = escape_table_cell(&row.help);
        let possible_values = mono_csv(&row.possible_values);
        let default = mono_csv(&row.default_values);
        table.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row.name, help, possible_values, default
        ));
    }
    table
}

/// Join values together as monospace, e.g. ["a", "b", "c"] -> `a`, `b`, `c`
fn mono_csv(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("`{}`", escape_table_cell(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// TODO we may need more comprehensive markdown escaping here
fn escape_table_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', "<br>")
}
