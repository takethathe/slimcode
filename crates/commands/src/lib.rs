//! slimcode-commands: frontend-agnostic slash-command registry and prediction.
//!
//! A `Command` is the canonical definition of a `/xxx` slash command: its
//! canonical name, aliases, usage string (with argument placeholder), and a
//! short description. `COMMANDS` is the single source of truth; `suggest`
//! turns a partial `/` input into matching commands (predictive hint), and
//! `find` resolves an exact spelling to its command.
//!
//! The crate is pure data + pure functions with no I/O, so any frontend (the
//! one-shot CLI, the TUI, a web UI, ...) can reuse the same
//! registry, `/help` text, and prediction logic. Installed *skills* are a
//! separate, dynamic `/` trigger set (`slimcode-app::skills`); the frontend
//! combines both when predicting partial `/` input.

pub mod fuzzy;

/// How a command matches input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    /// Matches its canonical name or one of its aliases (exact / prefix).
    Exact,
    /// Matches `name` or `name` followed by a digit sequence (e.g. `/!3`).
    Numbered,
}

/// A slash command known to an interactive frontend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Display form, argument placeholder included (e.g. `/load <id>`).
    pub usage: &'static str,
    pub description: &'static str,
    pub kind: CommandKind,
}

impl Command {
    /// An exact command with no aliases.
    pub const fn new(name: &'static str, usage: &'static str, description: &'static str) -> Self {
        Self {
            name,
            aliases: &[],
            usage,
            description,
            kind: CommandKind::Exact,
        }
    }

    /// An exact command with aliases.
    pub const fn aliased(
        name: &'static str,
        aliases: &'static [&'static str],
        usage: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            name,
            aliases,
            usage,
            description,
            kind: CommandKind::Exact,
        }
    }

    /// A numbered command: matches `name` or `name` plus digits.
    pub const fn numbered(
        name: &'static str,
        usage: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            name,
            aliases: &[],
            usage,
            description,
            kind: CommandKind::Numbered,
        }
    }

    /// All spellings that resolve to this command: canonical name first, then
    /// aliases.
    pub fn spellings(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }
}

/// The canonical command registry.
pub const COMMANDS: &[Command] = &[
    Command::new("/help", "/help", "list commands"),
    Command::new("/new", "/new", "start a new session"),
    Command::aliased("/load", &["/resume"], "/load <id>", "load a saved session"),
    Command::new("/sessions", "/sessions", "list saved sessions"),
    Command::new("/usage", "/usage", "show token usage"),
    Command::new("/history", "/history", "list input history"),
    Command::new("/skills", "/skills", "list installed skills"),
    Command::new(
        "/install-skill",
        "/install-skill <path> --user|--project",
        "install a skill (user or project scope)",
    ),
    Command::new("/!!", "/!!", "rerun the most recent prompt"),
    Command::numbered("/!", "/!N", "rerun history entry N (1 = newest)"),
    Command::aliased("/exit", &["/quit"], "/exit", "quit the TUI"),
];

/// Resolve an exact spelling (canonical name or alias) to its command.
pub fn find(input: &str) -> Option<&'static Command> {
    let trimmed = input.trim();
    COMMANDS
        .iter()
        .find(|c| c.spellings().any(|s| s == trimmed))
}

/// Predict matching commands for a partial `/` input. A non-`/` input yields
/// no suggestions; `/` alone yields every command.
pub fn suggest(input: &str) -> Vec<&'static Command> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|c| command_matches(c, trimmed))
        .collect()
}

/// Does `input` match this command under its kind's rules?
fn command_matches(c: &Command, input: &str) -> bool {
    match c.kind {
        CommandKind::Exact => c.spellings().any(|s| s.starts_with(input)),
        CommandKind::Numbered => {
            // The command's own name is a prefix (e.g. `/!` -> `/!N`), or the
            // input is the name followed by a digit sequence (e.g. `/!3`).
            c.name.starts_with(input)
                || input
                    .strip_prefix(c.name)
                    .is_some_and(|rest| rest.chars().all(|ch| ch.is_ascii_digit()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_bare_slash_returns_all() {
        let all = suggest("/");
        assert_eq!(all.len(), COMMANDS.len());
    }

    #[test]
    fn suggest_prefix_matches_name() {
        let got = suggest("/hist");
        let names: Vec<_> = got.iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["/history"]);
    }

    #[test]
    fn suggest_prefix_matches_alias() {
        let got = suggest("/res");
        let names: Vec<_> = got.iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["/load"]);
    }

    #[test]
    fn suggest_exact_spelling() {
        let got = suggest("/usage");
        let names: Vec<_> = got.iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["/usage"]);
    }

    #[test]
    fn suggest_case_sensitive() {
        assert!(suggest("/HIST").is_empty());
    }

    #[test]
    fn suggest_non_slash_input_is_empty() {
        assert!(suggest("hist").is_empty());
        assert!(suggest("").is_empty());
    }

    #[test]
    fn suggest_unknown_is_empty() {
        assert!(suggest("/zzz").is_empty());
    }

    #[test]
    fn suggest_trims_surrounding_whitespace() {
        let got = suggest("  /hist  ");
        let names: Vec<_> = got.iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["/history"]);
    }

    #[test]
    fn suggest_numbered_replay_with_digits() {
        let got = suggest("/!3");
        let names: Vec<_> = got.iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["/!"]);
    }

    #[test]
    fn suggest_numbered_replay_bare_prefix() {
        let got = suggest("/!");
        let usages: Vec<_> = got.iter().map(|c| c.usage).collect();
        assert!(usages.contains(&"/!N"), "got: {usages:?}");
        assert!(usages.contains(&"/!!"), "got: {usages:?}");
    }

    #[test]
    fn suggest_numbered_replay_rejects_non_digits() {
        assert!(suggest("/!abc").is_empty());
        assert!(
            suggest("/!")
                .iter()
                .all(|c| c.name == "/!" || c.name == "/!!")
        );
    }

    #[test]
    fn find_resolves_name_and_alias() {
        assert_eq!(find("/load").map(|c| c.name), Some("/load"));
        assert_eq!(find("/resume").map(|c| c.name), Some("/load"));
        assert_eq!(find("/quit").map(|c| c.name), Some("/exit"));
        assert!(find("/nope").is_none());
    }

    #[test]
    fn find_trims_and_is_case_sensitive() {
        assert_eq!(find(" /help ").map(|c| c.name), Some("/help"));
        assert!(find("/HELP").is_none());
    }

    #[test]
    fn registry_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in COMMANDS {
            for spelling in c.spellings() {
                assert!(seen.insert(spelling), "duplicate spelling: {spelling}");
            }
        }
    }

    #[test]
    fn usage_lines_cover_all_commands() {
        for c in COMMANDS {
            assert!(c.usage.starts_with('/'), "bad usage: {}", c.usage);
            assert!(!c.description.is_empty(), "no description for {}", c.name);
        }
    }
}
