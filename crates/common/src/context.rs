//! Context assembly: one turn's message list built from a base system prompt,
//! an advertised skills list, an optional message history, and a user prompt
//! or skill trigger.
//!
//! Frontend-agnostic: any frontend (the current line-based REPL, a future TUI,
//! or a web UI) builds a turn through [`ContextBuilder`] and hands the result
//! straight to `run_agent_from_messages`. See `.scratch/context-builder` spec.

use slimcode_agent::session::{Message, Role};

use crate::skills::{Skill, skill_prompt};

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

/// Assemble the full system prompt: the base grounding plus a list of
/// auto-invokable skills (those without `disable-model-invocation: true`).
/// Skills marked `disable-model-invocation` stay out of the system prompt and
/// are only reachable through an explicit `/name` trigger.
fn build_system_prompt(base: &str, skills: &[Skill]) -> String {
    let mut prompt = base.to_string();
    let auto: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if !auto.is_empty() {
        prompt.push_str("\n\n## Available skills\n\n");
        prompt.push_str("Enter the `/name` as a command to apply it:\n\n");
        for s in auto {
            prompt.push_str(&format!(
                "- `/{name}` — {desc}\n",
                name = s.name,
                desc = s.description
            ));
        }
    }
    prompt
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
    user: Option<String>,
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
        self.user = Some(prompt.into());
        self
    }

    /// Set this turn's user message from a skill trigger, reusing
    /// [`crate::skills::skill_prompt`] to produce the message text.
    pub fn with_skill(mut self, skill: &Skill, arg: Option<&str>) -> Self {
        self.user = Some(skill_prompt(skill, arg));
        self
    }

    /// Assemble the message list. Errors when no user content is set.
    ///
    /// Semantics (aligned with the CLI's former `messages_for_prompt`):
    /// - The final system text is the base system (default or overridden) plus
    ///   an `## Available skills` section for auto-invokable skills.
    /// - An empty history seeds exactly one leading `Role::System` message;
    ///   a non-empty history is not re-seeded.
    /// - A `Role::User` message (prompt or skill trigger) is appended last.
    pub fn build(self) -> Result<Vec<Message>, String> {
        let user = self.user.ok_or_else(|| {
            "no user message set: call with_user_prompt or with_skill".to_string()
        })?;
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
        assert!(system.contains("## Available skills"), "got: {system}");
        assert!(
            system.contains("- `/auto` — runs automatically"),
            "got: {system}"
        );
        assert!(!system.contains("manual"), "got: {system}");
        assert!(!system.contains("only on demand"));
    }

    #[test]
    fn skills_section_is_markdown() {
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
        // Heading + blank line + instruction + blank line + bullet list.
        assert!(system.contains("## Available skills\n\n"), "got: {system}");
        assert!(
            system.contains("Enter the `/name` as a command to apply it:\n\n"),
            "got: {system}"
        );
        assert!(
            system.contains("- `/hist` — history-ish\n"),
            "got: {system}"
        );
        // The base grounding (markdown) is still present before the skills
        // heading.
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
        assert!(!system.contains("Available skills"), "got: {system}");
        assert!(system.contains("read"));
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
        assert!(user.contains("demo"), "got: {user}");
        assert!(!user.contains("Task:"), "got: {user}");
    }

    #[test]
    fn with_skill_embeds_skill_directory() {
        let s = Skill {
            name: "demo".to_string(),
            description: "A demo skill".to_string(),
            disable_model_invocation: false,
            body: "Do the demo.".to_string(),
            scope: SkillScope::User,
            dir: std::path::PathBuf::from("/tmp/skills/demo"),
        };
        let messages = ContextBuilder::new()
            .with_system("sys")
            .with_skill(&s, None)
            .build()
            .unwrap();
        let user = messages[1].text_content();
        assert!(
            user.contains("Skill directory: /tmp/skills/demo"),
            "got: {user}"
        );
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
        assert!(user.ends_with("Task: run it now"), "got: {user}");
    }

    #[test]
    fn build_errors_without_user_message() {
        let err = ContextBuilder::new().build().unwrap_err();
        assert!(err.contains("user message"), "err: {err}");
    }
}
