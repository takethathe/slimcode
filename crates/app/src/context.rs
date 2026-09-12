//! Context assembly: one turn's message list built from a base system prompt,
//! an advertised skills list, an optional message history, and a user prompt
//! or skill trigger.
//!
//! Frontend-agnostic: any frontend (the one-shot CLI, the TUI, or a web UI)
//! builds a turn through [`ContextBuilder`] and hands the result straight to
//! `run_agent_from_messages`. See `.scratch/context-builder` spec.

use std::path::PathBuf;

use slimcode_core::session::{AgentMessage, Message, Role};

use crate::context_files::{ContextFile, format_context_files};
use crate::skills::{Skill, format_skills_for_prompt};

/// Default base system prompt grounding the agent in its tools and working
/// directory. Skill descriptions are appended on top by the builder's `build()`.
pub const DEFAULT_SYSTEM_PROMPT: &str = concat!(
    "You are slimcode, a coding agent that works in a repository directory.\n\n",
    "## Tools\n\n",
    "You have these tools: `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`.\n\n",
    "## Working style\n\n",
    "- Plan with the tools available: inspect files before editing, run commands ",
    "to verify, and complete the user's task.\n",
    "- Keep answers concise.\n",
    "- When a tool fails, read the error and retry with a corrected approach."
);

/// System environment info injected into the system prompt's `## Environment`
/// section: the OS name, the global home (the slimcode home dir), and the
/// project home (the git repository root, or the OS user home as a fallback).
/// The frontends resolve these at startup and pass them via
/// [`ContextBuilder::with_environment`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    /// OS name (`std::env::consts::OS`): "macos", "linux", "windows".
    pub os: String,
    /// Global home: the slimcode home dir (`$SLIMCODE_HOME` or `~/.slimcode`),
    /// where global context files and skills live.
    pub global_home: PathBuf,
    /// Project home: the nearest ancestor of the working directory holding a
    /// `.git` entry, falling back to the OS user home (`$HOME`) when no
    /// ancestor is a git repo.
    pub project_home: PathBuf,
}

/// Render the `## Environment` markdown section for the system prompt: one
/// bullet per field (OS, global home, project home), so the model knows the
/// platform and where the global/project roots live without probing the
/// filesystem. Starts with a blank-line separator (aligned with
/// [`format_context_files`]) and ends with a single newline, so the following
/// `## Project context` / `## Skills` section sits one blank line below.
fn format_environment(env: &Environment) -> String {
    format!(
        "\n\n## Environment\n\n- OS: {}\n- global home: {}\n- project home: {}\n",
        env.os,
        env.global_home.display(),
        env.project_home.display(),
    )
}

/// Assemble the full system prompt: the base grounding plus an optional
/// `## Environment` markdown section of OS / global home / project home (when
/// [`ContextBuilder::with_environment`] was called), plus a
/// `## Project context` markdown section of context files (global + project
/// `AGENTS.md`, pi-style, with scope-labelled XML blocks and a
/// project-overrides-global note), plus a `## Skills` markdown index of
/// auto-invokable skills (those without `disable-model-invocation: true`),
/// each bullet carrying its name, description, and the `SKILL.md` file the
/// model can `read`. Skills marked
/// `disable-model-invocation` stay out of the system prompt and are only
/// reachable through an explicit `/skill:name` trigger.
fn build_system_prompt(
    base: &str,
    environment: Option<&Environment>,
    skills: &[Skill],
    context_files: &[ContextFile],
) -> String {
    let mut prompt = base.to_string();
    if let Some(env) = environment {
        prompt.push_str(&format_environment(env));
    }
    prompt.push_str(&format_context_files(context_files));
    prompt.push_str(&format_skills_for_prompt(skills));
    prompt
}

/// Has this skill's `<skill name="...">` block already been injected into one
/// of the history messages? Detected from the XML wrapper the trigger inserts,
/// so the check is stateless and survives session reloads.
pub fn skill_loaded_in(history: &[AgentMessage], skill: &Skill) -> bool {
    let marker = format!("<skill name=\"{}\"", skill.name);
    history.iter().any(|m| m.text_content().contains(&marker))
}

/// One turn's assembled context (ADR-0012 D3): the system message is
/// assembled fresh from live state and handed to the runtime separately; only
/// the `messages` (history plus this turn's prompt) ever enter a Session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    /// The system message for this turn — never stored in a Session.
    pub system: Message,
    /// The history plus this turn's user message.
    pub messages: Vec<AgentMessage>,
}

/// A fluent builder for one turn's context.
///
/// Components are optional and added as needed: the system prompt defaults to
/// [`DEFAULT_SYSTEM_PROMPT`] (overridable via [`ContextBuilder::with_system`]),
/// environment info and context files are injected only when supplied, and a
/// non-empty history is kept as-is. `build()` returns a [`Context`] whose
/// system message is separate from the messages, so a turn adds only its
/// prompt message to history.
#[derive(Debug)]
pub struct ContextBuilder {
    system: String,
    environment: Option<Environment>,
    skills: Vec<Skill>,
    context_files: Vec<ContextFile>,
    history: Vec<AgentMessage>,
    user: Option<String>,
}

impl ContextBuilder {
    /// Start with the default system prompt and no other components.
    pub fn new() -> Self {
        Self {
            system: DEFAULT_SYSTEM_PROMPT.to_string(),
            environment: None,
            skills: Vec::new(),
            context_files: Vec::new(),
            history: Vec::new(),
            user: None,
        }
    }

    /// Override the base system prompt.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self
    }

    /// Inject system environment info (OS, global home, project home) as a
    /// `## Environment` markdown section between the base prompt and the
    /// context files. Skipped entirely when not called, so the default system
    /// prompt stays byte-identical.
    pub fn with_environment(mut self, environment: Environment) -> Self {
        self.environment = Some(environment);
        self
    }

    /// Advertise a skills list (auto-invokable skills only, filtered at build).
    pub fn with_skills(mut self, skills: &[Skill]) -> Self {
        self.skills = skills.to_vec();
        self
    }

    /// Inject context files (`AGENTS.md`, global + project, pi-style) into
    /// the system prompt between the base and the skills index.
    pub fn with_context_files(mut self, files: &[ContextFile]) -> Self {
        self.context_files = files.to_vec();
        self
    }

    /// Continue from an existing message history. The system message is not
    /// part of it (ADR-0012 D3) and is assembled per request.
    pub fn with_history(mut self, history: Vec<AgentMessage>) -> Self {
        self.history = history;
        self
    }

    /// Set this turn's user message. A skill trigger is a user message too:
    /// the CLI renders it with [`crate::skills::skill_prompt`] (deduped against
    /// the history via [`skill_loaded_in`]) and passes the text here, so
    /// command and skill semantics stay on the CLI side (ADR-0013 D3).
    pub fn with_user_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.user = Some(prompt.into());
        self
    }

    /// Assemble the turn's context. Errors when no user content is set.
    ///
    /// Semantics (aligned with the CLI's former `messages_for_prompt`):
    /// - The system message is the base system (default or overridden) plus
    ///   the `## Environment` / `## Project context` / `## Skills` sections
    ///   for whatever was supplied. It is returned separately and never enters
    ///   the message list (ADR-0012 D3).
    /// - The message list is the supplied history followed by exactly one
    ///   `Role::User` message: the caller's prompt text, or a skill trigger it
    ///   rendered itself.
    pub fn build(self) -> Result<Context, String> {
        let user = self
            .user
            .ok_or_else(|| "no user message set: call with_user_prompt".to_string())?;
        let system = Message::text(
            Role::System,
            build_system_prompt(
                &self.system,
                self.environment.as_ref(),
                &self.skills,
                &self.context_files,
            ),
        );
        let mut messages = self.history;
        messages.push(AgentMessage::text(Role::User, user));
        Ok(Context { system, messages })
    }
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillScope;

    fn skill(name: &str, desc: &str, disable: bool) -> Skill {
        Skill {
            name: name.to_string(),
            description: desc.to_string(),
            disable_model_invocation: disable,
            body: String::new(),
            scope: SkillScope::User,
            dir: std::path::PathBuf::new(),
            file: std::path::PathBuf::new(),
        }
    }

    #[test]
    fn default_system_prompt_is_used_without_override() {
        let context = ContextBuilder::new()
            .with_user_prompt("hello")
            .build()
            .unwrap();
        // The system message is separate from the history (ADR-0012 D3): a
        // turn adds only its user message.
        assert_eq!(context.messages.len(), 1);
        assert_eq!(context.system.role, Role::System);
        assert_eq!(context.system.text_content(), DEFAULT_SYSTEM_PROMPT);
        assert_eq!(context.messages[0].role(), &Role::User);
        assert_eq!(context.messages[0].text_content(), "hello");
    }

    #[test]
    fn with_system_overrides_default() {
        let context = ContextBuilder::new()
            .with_system("be a coder")
            .with_user_prompt("hello")
            .build()
            .unwrap();
        assert_eq!(context.messages.len(), 1);
        assert_eq!(context.system.role, Role::System);
        assert_eq!(context.system.text_content(), "be a coder");
        assert_eq!(context.messages[0].text_content(), "hello");
    }

    #[test]
    fn empty_history_seeds_only_the_user_message() {
        let context = ContextBuilder::new()
            .with_system("be a coder")
            .with_user_prompt("hello")
            .build()
            .unwrap();
        assert_eq!(context.messages.len(), 1);
        assert_eq!(context.system.text_content(), "be a coder");
        assert_eq!(context.messages[0].role(), &Role::User);
        assert_eq!(context.messages[0].text_content(), "hello");
    }

    #[test]
    fn non_empty_history_is_kept_and_system_stays_separate() {
        let history = vec![AgentMessage::text(Role::Assistant, "earlier")];
        let context = ContextBuilder::new()
            .with_system("live system")
            .with_history(history)
            .with_user_prompt("again")
            .build()
            .unwrap();
        assert_eq!(context.system.text_content(), "live system");
        assert_eq!(context.messages.len(), 2);
        assert_eq!(context.messages[0].role(), &Role::Assistant);
        assert_eq!(context.messages[0].text_content(), "earlier");
        assert_eq!(context.messages[1].text_content(), "again");
    }

    #[test]
    fn system_advertises_only_auto_invokable_skills() {
        let skills = vec![
            skill("auto", "runs automatically", false),
            skill("manual", "only on demand", true),
        ];
        let context = ContextBuilder::new()
            .with_skills(&skills)
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        assert!(system.contains("## Skills"), "got: {system}");
        assert!(
            system.contains("- auto: runs automatically [Read from "),
            "got: {system}"
        );
        assert!(!system.contains("manual"), "got: {system}");
        assert!(!system.contains("only on demand"));
    }

    #[test]
    fn skills_section_is_markdown_skill_index() {
        let skills = vec![
            skill("auto", "runs automatically", false),
            skill("hist", "history-ish", false),
        ];
        let context = ContextBuilder::new()
            .with_skills(&skills)
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        assert!(system.contains("## Skills"), "got: {system}");
        assert!(
            system.contains(
                "Use a skill when its name or description matches the task, or when the user \
                 references it explicitly as /{name}."
            ),
            "got: {system}"
        );
        assert!(
            system.contains("Read the file and follow its instructions."),
            "got: {system}"
        );
        assert!(
            system.contains("- hist: history-ish [Read from "),
            "got: {system}"
        );
        assert!(
            system.contains("- auto: runs automatically [Read from "),
            "got: {system}"
        );
        assert!(system.contains("You are slimcode"), "got: {system}");
    }

    #[test]
    fn default_system_prompt_is_markdown_structured() {
        let prompt = DEFAULT_SYSTEM_PROMPT;
        assert!(
            prompt.contains(
                "You are slimcode, a coding agent that works in a repository directory.\n\n"
            ),
            "got: {prompt}"
        );
        assert!(prompt.contains("## Tools\n\n"), "got: {prompt}");
        assert!(prompt.contains("`read`"), "got: {prompt}");
        assert!(prompt.contains("`bash`"), "got: {prompt}");
        assert!(prompt.contains("## Working style\n\n"), "got: {prompt}");
        assert!(
            prompt.contains("- Keep answers concise.\n"),
            "got: {prompt}"
        );
    }

    #[test]
    fn no_skills_has_no_skills_section() {
        let context = ContextBuilder::new()
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        assert!(!system.contains("## Skills"), "got: {system}");
        assert!(system.contains("read"));
    }

    #[test]
    fn system_prompt_injects_descriptions_of_nested_skills() {
        use crate::skills::SkillStore;
        use crate::testutil::unique_temp_dir;
        use std::fs;

        let dir = unique_temp_dir("slimcode-context-skills");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        fs::create_dir_all(home.join("skills").join("engineering").join("tdd")).unwrap();
        fs::create_dir_all(home.join("skills").join("design").join("grill")).unwrap();
        fs::write(
            home.join("skills")
                .join("engineering")
                .join("tdd")
                .join("SKILL.md"),
            "---\nname: tdd\ndescription: test first\n---\nred green refactor\n",
        )
        .unwrap();
        fs::write(
            home.join("skills")
                .join("design")
                .join("grill")
                .join("SKILL.md"),
            "---\nname: grill\ndescription: stress-test a plan\n---\ninterview\n",
        )
        .unwrap();

        let store = SkillStore::new(&home, &cwd);
        let skills = store.list().unwrap();
        let context = ContextBuilder::new()
            .with_skills(&skills)
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        assert!(
            system.contains("- tdd: test first [Read from "),
            "got: {system}"
        );
        assert!(
            system.contains("- grill: stress-test a plan [Read from "),
            "got: {system}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_user_prompt_sets_user_message() {
        let context = ContextBuilder::new()
            .with_system("sys")
            .with_user_prompt("do the task")
            .build()
            .unwrap();
        assert_eq!(context.messages[0].role(), &Role::User);
        assert_eq!(context.messages[0].text_content(), "do the task");
    }

    #[test]
    fn skill_loaded_in_detects_the_xml_marker() {
        let s = skill("demo", "A demo skill", false);
        assert!(!skill_loaded_in(&[], &s));
        let history = vec![AgentMessage::text(
            Role::User,
            "please run <skill name=\"demo\"> the demo",
        )];
        assert!(skill_loaded_in(&history, &s));
        // A different skill's marker does not count.
        assert!(!skill_loaded_in(&history, &skill("other", "x", false)));
    }

    #[test]
    fn build_errors_without_user_message() {
        let err = ContextBuilder::new().build().unwrap_err();
        assert!(err.contains("user message"), "err: {err}");
    }

    // --- context files (AGENTS.md) injection ------------------------------

    #[test]
    fn context_files_inject_between_base_and_skills() {
        use crate::context_files::ContextScope;
        use std::path::PathBuf;

        let files = vec![ContextFile {
            path: PathBuf::from("/tmp/AGENTS.md"),
            content: "do the thing".to_string(),
            scope: ContextScope::Project,
        }];
        let skills = vec![skill("auto", "runs automatically", false)];
        let context = ContextBuilder::new()
            .with_context_files(&files)
            .with_skills(&skills)
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        let base = system.find("You are slimcode").unwrap();
        let ctx = system.find("## Project context").unwrap();
        let skills_idx = system.find("## Skills").unwrap();
        assert!(base < ctx, "context files must follow the base prompt");
        assert!(ctx < skills_idx, "skills index must follow context files");
        assert!(system.contains("scope=\"project\""), "got: {system}");
        assert!(system.contains("do the thing"), "got: {system}");
        assert!(
            system.contains("project requirements override global requirements"),
            "got: {system}"
        );
    }

    #[test]
    fn no_context_files_has_no_project_context_section() {
        let context = ContextBuilder::new()
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        assert!(!system.contains("## Project context"), "got: {system}");
        assert!(!system.contains("AGENTS.md"), "got: {system}");
    }

    // --- environment info (## Environment) -------------------------------

    fn environment() -> Environment {
        Environment {
            os: "macos".to_string(),
            global_home: std::path::PathBuf::from("/home/u/.slimcode"),
            project_home: std::path::PathBuf::from("/home/u/work/repo"),
        }
    }

    #[test]
    fn no_environment_has_no_environment_section() {
        let context = ContextBuilder::new()
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        assert!(!system.contains("## Environment"), "got: {system}");
        assert!(!system.contains("global home"), "got: {system}");
    }

    #[test]
    fn environment_injects_between_base_and_context_files() {
        use crate::context_files::ContextScope;
        use std::path::PathBuf;

        let files = vec![ContextFile {
            path: PathBuf::from("/tmp/AGENTS.md"),
            content: "do the thing".to_string(),
            scope: ContextScope::Project,
        }];
        let context = ContextBuilder::new()
            .with_environment(environment())
            .with_context_files(&files)
            .with_skills(&[skill("auto", "runs automatically", false)])
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        let base = system.find("You are slimcode").unwrap();
        let env = system.find("## Environment").unwrap();
        let ctx = system.find("## Project context").unwrap();
        let skills_idx = system.find("## Skills").unwrap();
        assert!(base < env, "environment must follow the base prompt");
        assert!(env < ctx, "environment must precede context files");
        assert!(ctx < skills_idx, "skills index must follow context files");
        assert!(system.contains("- OS: macos"), "got: {system}");
        assert!(
            system.contains("- global home: /home/u/.slimcode"),
            "got: {system}"
        );
        assert!(
            system.contains("- project home: /home/u/work/repo"),
            "got: {system}"
        );
    }

    #[test]
    fn environment_section_precedes_skills_without_context_files() {
        let context = ContextBuilder::new()
            .with_environment(environment())
            .with_skills(&[skill("auto", "runs automatically", false)])
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = context.system.text_content();
        let env = system.find("## Environment").unwrap();
        let skills_idx = system.find("## Skills").unwrap();
        assert!(env < skills_idx, "got: {system}");
        assert!(system.contains("- OS: macos"), "got: {system}");
        assert!(
            system.contains("- global home: /home/u/.slimcode"),
            "got: {system}"
        );
        assert!(
            system.contains("- project home: /home/u/work/repo"),
            "got: {system}"
        );
    }
}
