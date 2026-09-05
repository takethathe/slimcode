//! Context assembly: one turn's message list built from a base system prompt,
//! an advertised skills list, an optional message history, and a user prompt
//! or skill trigger.
//!
//! Frontend-agnostic: any frontend (the one-shot CLI, the TUI, or a web UI)
//! builds a turn through [`ContextBuilder`] and hands the result straight to
//! `run_agent_from_messages`. See `.scratch/context-builder` spec.

use slimcode_agent::session::{Message, Role};

use crate::skills::{Skill, format_skills_for_prompt, skill_prompt};

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

/// Assemble the full system prompt: the base grounding plus a `## Skills`
/// markdown index of auto-invokable skills (those without
/// `disable-model-invocation: true`), each bullet carrying its name,
/// description, and the `SKILL.md` file the model can `read`. Skills marked
/// `disable-model-invocation` stay out of the system prompt and are only
/// reachable through an explicit `/skill:name` trigger.
fn build_system_prompt(base: &str, skills: &[Skill]) -> String {
    let mut prompt = base.to_string();
    prompt.push_str(&format_skills_for_prompt(skills));
    prompt
}

/// One turn's user message: either a plain prompt or a skill trigger. The
/// skill body is rendered at `build()` time so a skill already loaded in an
/// earlier message of the history can be replaced by an already-loaded notice
/// instead of being repeated.
#[derive(Debug)]
enum UserInput {
    Prompt(String),
    Skill { skill: Skill, arg: Option<String> },
}

/// Has this skill's `<skill name="...">` block already been injected into one
/// of the history messages? Detected from the XML wrapper the trigger inserts,
/// so the check is stateless and survives session reloads.
fn skill_loaded_in(history: &[Message], skill: &Skill) -> bool {
    let marker = format!("<skill name=\"{}\"", skill.name);
    history.iter().any(|m| m.text_content().contains(&marker))
}

/// A fluent builder for one turn's message list.
///
/// Components are optional and added as needed: the system prompt defaults to
/// [`DEFAULT_SYSTEM_PROMPT`] (overridable via [`ContextBuilder::with_system`]),
/// skills are advertised only when a fresh system message is seeded, and a
/// non-empty history is never re-seeded. `build()` returns the assembled
/// `Vec<Message>`, ready for `run_agent_from_messages`.
#[derive(Debug)]
pub struct ContextBuilder {
    system: String,
    skills: Vec<Skill>,
    history: Vec<Message>,
    user: Option<UserInput>,
}

impl ContextBuilder {
    /// Start with the default system prompt and no other components.
    pub fn new() -> Self {
        Self {
            system: DEFAULT_SYSTEM_PROMPT.to_string(),
            skills: Vec::new(),
            history: Vec::new(),
            user: None,
        }
    }

    /// Override the base system prompt.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self
    }

    /// Advertise a skills list (auto-invokable skills only, filtered at build).
    pub fn with_skills(mut self, skills: &[Skill]) -> Self {
        self.skills = skills.to_vec();
        self
    }

    /// Continue from an existing message history. A non-empty history is not
    /// re-seeded with a system message.
    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        self.history = history;
        self
    }

    /// Set this turn's user message from a plain prompt.
    pub fn with_user_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.user = Some(UserInput::Prompt(prompt.into()));
        self
    }

    /// Set this turn's user message from a skill trigger. The skill body is
    /// rendered at `build()` time via [`crate::skills::skill_prompt`], so a
    /// skill already loaded in the supplied history is replaced by an
    /// already-loaded notice instead of being repeated.
    pub fn with_skill(mut self, skill: &Skill, arg: Option<&str>) -> Self {
        self.user = Some(UserInput::Skill {
            skill: skill.clone(),
            arg: arg.map(str::to_string),
        });
        self
    }

    /// Assemble the message list. Errors when no user content is set.
    ///
    /// Semantics (aligned with the CLI's former `messages_for_prompt`):
    /// - The final system text is the base system (default or overridden) plus
    ///   the `## Skills` markdown index for auto-invokable skills.
    /// - An empty history seeds exactly one leading `Role::System` message;
    ///   a non-empty history is not re-seeded.
    /// - A `Role::User` message (prompt or skill trigger) is appended last; a
    ///   skill trigger whose `<skill name="...">` block already appears in the
    ///   history is deduplicated (body replaced, base-dir reference kept).
    pub fn build(self) -> Result<Vec<Message>, String> {
        let user = match self.user {
            Some(UserInput::Prompt(prompt)) => prompt,
            Some(UserInput::Skill { skill, arg }) => {
                let already_loaded = skill_loaded_in(&self.history, &skill);
                skill_prompt(&skill, arg.as_deref(), already_loaded)
            }
            None => {
                return Err("no user message set: call with_user_prompt or with_skill".to_string());
            }
        };
        let base = self.system;
        let mut messages = self.history;
        if messages.is_empty() {
            messages.push(Message::text(
                Role::System,
                build_system_prompt(&base, &self.skills),
            ));
        }
        messages.push(Message::text(Role::User, user));
        Ok(messages)
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
        let messages = ContextBuilder::new()
            .with_user_prompt("hello")
            .build()
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].text_content(), DEFAULT_SYSTEM_PROMPT);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].text_content(), "hello");
    }

    #[test]
    fn with_system_overrides_default() {
        let messages = ContextBuilder::new()
            .with_system("be a coder")
            .with_user_prompt("hello")
            .build()
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].text_content(), "be a coder");
        assert_eq!(messages[1].text_content(), "hello");
    }

    #[test]
    fn empty_history_seeds_system_message() {
        let messages = ContextBuilder::new()
            .with_system("be a coder")
            .with_user_prompt("hello")
            .build()
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].text_content(), "be a coder");
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].text_content(), "hello");
    }

    #[test]
    fn non_empty_history_not_reseeded() {
        let history = vec![Message::text(Role::System, "sys")];
        let messages = ContextBuilder::new()
            .with_history(history)
            .with_user_prompt("again")
            .build()
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].text_content(), "sys");
        assert_eq!(messages[1].text_content(), "again");
    }

    #[test]
    fn system_advertises_only_auto_invokable_skills() {
        let skills = vec![
            skill("auto", "runs automatically", false),
            skill("manual", "only on demand", true),
        ];
        let messages = ContextBuilder::new()
            .with_skills(&skills)
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = messages[0].text_content();
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
        let messages = ContextBuilder::new()
            .with_skills(&skills)
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = messages[0].text_content();
        // The `## Skills` header + one markdown bullet per skill.
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
        // The base grounding (markdown) is still present before the skills
        // section.
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
        let messages = ContextBuilder::new()
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = messages[0].text_content();
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
        let messages = ContextBuilder::new()
            .with_skills(&skills)
            .with_user_prompt("hello")
            .build()
            .unwrap();
        let system = messages[0].text_content();
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
        let messages = ContextBuilder::new()
            .with_system("sys")
            .with_user_prompt("do the task")
            .build()
            .unwrap();
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[1].text_content(), "do the task");
    }

    #[test]
    fn with_skill_sets_user_message_from_skill() {
        let s = skill("demo", "A demo skill", false);
        let messages = ContextBuilder::new()
            .with_system("sys")
            .with_skill(&s, None)
            .build()
            .unwrap();
        let user = messages[1].text_content();
        assert!(user.starts_with("<skill name=\"demo\""), "got: {user}");
        assert!(user.ends_with("</skill>"), "got: {user}");
        assert!(!user.contains("Task:"), "got: {user}");
    }

    #[test]
    fn with_skill_embeds_skill_directory_and_file() {
        let s = Skill {
            name: "demo".to_string(),
            description: "A demo skill".to_string(),
            disable_model_invocation: false,
            body: "Do the demo.".to_string(),
            scope: SkillScope::User,
            dir: std::path::PathBuf::from("/tmp/skills/demo"),
            file: std::path::PathBuf::from("/tmp/skills/demo/SKILL.md"),
        };
        let messages = ContextBuilder::new()
            .with_system("sys")
            .with_skill(&s, None)
            .build()
            .unwrap();
        let user = messages[1].text_content();
        assert!(
            user.contains("References are relative to /tmp/skills/demo."),
            "got: {user}"
        );
        assert!(
            user.contains("location=\"/tmp/skills/demo/SKILL.md\""),
            "got: {user}"
        );
        assert!(user.contains("Do the demo."));
    }

    #[test]
    fn with_skill_embeds_task_argument() {
        let s = skill("demo", "A demo skill", false);
        let messages = ContextBuilder::new()
            .with_system("sys")
            .with_skill(&s, Some("run it now"))
            .build()
            .unwrap();
        let user = messages[1].text_content();
        assert!(user.ends_with("</skill>\n\nrun it now"), "got: {user}");
    }

    #[test]
    fn with_skill_dedupes_when_already_loaded_in_history() {
        let s = Skill {
            name: "demo".to_string(),
            description: "A demo skill".to_string(),
            disable_model_invocation: false,
            body: "Do the demo.".to_string(),
            scope: SkillScope::User,
            dir: std::path::PathBuf::from("/tmp/skills/demo"),
            file: std::path::PathBuf::from("/tmp/skills/demo/SKILL.md"),
        };
        // The skill was already loaded in an earlier user message: its body is
        // replaced by an already-loaded notice, but the base-dir reference line
        // is kept.
        let history = vec![Message::text(Role::User, skill_prompt(&s, None, false))];
        let messages = ContextBuilder::new()
            .with_system("sys")
            .with_history(history)
            .with_skill(&s, None)
            .build()
            .unwrap();
        let user = messages[1].text_content();
        assert!(
            user.contains("References are relative to /tmp/skills/demo."),
            "got: {user}"
        );
        assert!(user.contains("already loaded"), "got: {user}");
        assert!(!user.contains("Do the demo."), "got: {user}");
    }

    #[test]
    fn with_skill_not_deduped_when_absent_from_history() {
        let s = Skill {
            name: "demo".to_string(),
            description: "A demo skill".to_string(),
            disable_model_invocation: false,
            body: "Do the demo.".to_string(),
            scope: SkillScope::User,
            dir: std::path::PathBuf::from("/tmp/skills/demo"),
            file: std::path::PathBuf::from("/tmp/skills/demo/SKILL.md"),
        };
        // A different skill in history does not trigger dedup.
        let other = skill("other", "Other skill", false);
        let history = vec![Message::text(Role::User, skill_prompt(&other, None, false))];
        let messages = ContextBuilder::new()
            .with_system("sys")
            .with_history(history)
            .with_skill(&s, None)
            .build()
            .unwrap();
        let user = messages[1].text_content();
        assert!(user.contains("Do the demo."), "got: {user}");
        assert!(!user.contains("already loaded"), "got: {user}");
    }

    #[test]
    fn build_errors_without_user_message() {
        let err = ContextBuilder::new().build().unwrap_err();
        assert!(err.contains("user message"), "err: {err}");
    }
}
