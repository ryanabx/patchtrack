//! The book's generated pages: one per command, from its help text, and the
//! settings reference, from `patchtrack_core::settings::KEYS`. A command's
//! documentation is its help, so `--help` and the book say the same, and a
//! new command gets its page without anyone writing one.
//!
//! `patchtrack book-pages BOOK` (hidden) writes BOOK/src/commands/,
//! BOOK/src/settings.md and BOOK/src/SUMMARY.md, the last from
//! BOOK/SUMMARY.template.md with `{{commands}}` replaced.

use std::fmt::Write as _;
use std::path::Path;

use clap::Command;
use patchtrack_core::settings;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn visible(c: &Command) -> impl Iterator<Item = &Command> {
    c.get_subcommands().filter(|s| !s.is_hide_set() && s.get_name() != "help")
}

/// Markdown for one command: its description, usage, arguments, and its
/// own subcommands.
fn page(cmd: &mut Command, path: &str, level: usize) -> String {
    let mut out = String::new();
    let h = "#".repeat(level);
    let _ = writeln!(out, "{h} {path}\n");
    let about = cmd.get_long_about().or(cmd.get_about()).map(|a| a.to_string()).unwrap_or_default();
    let _ = writeln!(out, "{}\n", about.trim_end());
    let usage = cmd.render_usage().to_string();
    let _ = writeln!(out, "```text\n{}\n```\n", usage.trim_start_matches("Usage: "));
    // Global options are on the index, once.
    let args: Vec<_> = cmd
        .get_arguments()
        .filter(|a| !a.is_hide_set() && !a.is_global_set() && a.get_id() != "help" && a.get_id() != "version")
        .collect();
    if !args.is_empty() {
        let _ = writeln!(out, "| argument | |\n|---|---|");
        for a in args {
            let name = if a.is_positional() {
                format!("`{}`", a.get_value_names().map_or(a.get_id().as_str().to_uppercase(), |v| v.join(" ")))
            } else {
                let mut forms = vec![];
                if let Some(s) = a.get_short() {
                    forms.push(format!("-{s}"));
                }
                if let Some(l) = a.get_long() {
                    forms.push(format!("--{l}"));
                }
                let takes_value = takes_value(a);
                let value = a
                    .get_value_names()
                    .filter(|_| takes_value)
                    .map(|v| format!(" <{}>", v.join(" ")))
                    .unwrap_or_default();
                format!("`{}{value}`", forms.join(", "))
            };
            let help = a.get_long_help().or(a.get_help()).map(|h| h.to_string()).unwrap_or_default();
            let mut help = help.replace('\n', " ").replace('|', "\\|");
            let defaults: Vec<String> =
                a.get_default_values().iter().map(|v| v.to_string_lossy().into_owned()).collect();
            if !defaults.is_empty() && takes_value(a) {
                let _ = write!(help, " (default: `{}`)", defaults.join(", "));
            }
            let choices: Vec<String> = a.get_possible_values().iter().map(|p| format!("`{}`", p.get_name())).collect();
            if !choices.is_empty() && a.get_long() != Some("state") {
                let _ = write!(help, " (one of {})", choices.join(", "));
            }
            let _ = writeln!(out, "| {name} | {help} |");
        }
        out.push('\n');
    }
    let names: Vec<String> = visible(cmd).map(|s| s.get_name().to_string()).collect();
    for name in names {
        let sub = cmd.find_subcommand_mut(&name).expect("listed");
        out += &page(sub, &format!("{path} {name}"), level + 1);
    }
    out
}

fn takes_value(a: &clap::Arg) -> bool {
    a.get_num_args().is_some_and(|n| n.takes_values())
}

fn settings_page() -> String {
    let mut out = String::from(
        "# Settings\n\nSettings live in the store, not in a file: `patchtrack settings` shows them all, \
         `patchtrack settings set KEY VALUE` changes one, and `patchtrack settings unset KEY` puts it back. \
         Values are JSON, or a bare string. An exported store carries its settings; a public one does not. \
         Command-line options override a setting for one run.\n\n",
    );
    for k in settings::KEYS {
        let _ = writeln!(out, "## {}\n\n{}\n\n- shape: {}\n- default: `{}`\n", k.name, k.doc, k.shape, k.default);
    }
    out
}

pub fn write(mut cli: Command, book: &Path) -> Result<()> {
    cli.build();
    let src = book.join("src");
    let commands = src.join("commands");
    std::fs::create_dir_all(&commands)?;
    let names: Vec<(String, String)> = visible(&cli)
        .map(|c| (c.get_name().to_string(), c.get_about().map(|a| a.to_string()).unwrap_or_default()))
        .collect();
    let mut index = String::from(
        "# Commands\n\nEvery command explains itself: `patchtrack COMMAND --help` shows what these pages show.\n\n",
    );
    for a in cli.get_arguments().filter(|a| a.is_global_set() && !a.is_hide_set()) {
        let help = a.get_long_help().or(a.get_help()).map(|h| h.to_string()).unwrap_or_default().replace('\n', " ");
        let _ = writeln!(index, "Every command takes `--{}`: {help}.\n", a.get_long().unwrap_or_default());
    }
    let mut summary = String::new();
    for (name, about) in &names {
        let cmd = cli.find_subcommand_mut(name).expect("listed");
        std::fs::write(commands.join(format!("{name}.md")), page(cmd, &format!("patchtrack {name}"), 1))?;
        let _ = writeln!(index, "- [`{name}`]({name}.md): {}", about.trim_end_matches('.'));
        let _ = writeln!(summary, "  - [{name}](commands/{name}.md)");
    }
    std::fs::write(commands.join("index.md"), index)?;
    std::fs::write(src.join("settings.md"), settings_page())?;
    let template = std::fs::read_to_string(book.join("SUMMARY.template.md"))?;
    std::fs::write(src.join("SUMMARY.md"), template.replace("{{commands}}\n", &summary))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_command_gets_a_page_and_a_place_in_the_summary() {
        let book = std::env::temp_dir().join(format!("patchtrack-book-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&book);
        std::fs::create_dir_all(&book).unwrap();
        std::fs::write(
            book.join("SUMMARY.template.md"),
            "# Summary\n\n- [Commands](commands/index.md)\n{{commands}}\n",
        )
        .unwrap();
        super::write(crate::command(), &book).unwrap();
        let summary = std::fs::read_to_string(book.join("src/SUMMARY.md")).unwrap();
        let index = std::fs::read_to_string(book.join("src/commands/index.md")).unwrap();
        for c in crate::command().get_subcommands().filter(|c| !c.is_hide_set() && c.get_name() != "help") {
            let name = c.get_name();
            let page = std::fs::read_to_string(book.join(format!("src/commands/{name}.md"))).unwrap();
            assert!(page.starts_with(&format!("# patchtrack {name}\n")), "{name}");
            assert!(summary.contains(&format!("  - [{name}](commands/{name}.md)\n")), "{name}");
            assert!(index.contains(&format!("[`{name}`]({name}.md)")), "{name}");
        }
        assert!(!summary.contains("book-pages") && !summary.contains("{{commands}}"));
        let settings = std::fs::read_to_string(book.join("src/settings.md")).unwrap();
        assert!(patchtrack_core::settings::KEYS.iter().all(|k| settings.contains(&format!("\n## {}\n", k.name))));
        let _ = std::fs::remove_dir_all(&book);
    }
}
