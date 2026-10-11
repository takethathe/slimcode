//! Resource assembly — AGENTS.md context-file discovery, skills discovery +
//! the skill `read`-interception, explicit config crossing the `chat` seam,
//! and injected provider/tools taking effect (spec Testing Decisions items
//! 12, 13, 14; User Stories 11, 12, 13, 15, 16).
//!
//! All driven through the public `AgentSession` API only, with a scripted
//! `FakeProvider` injected via `AgentSessionOptions.provider` — the SDK never
//! exposes a test-only entry point.

mod common;
use common::*;

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use slimcode_app::context::DEFAULT_SYSTEM_PROMPT;
use slimcode_app::{
    CancelToken, Message, Provider, ProviderConfig, Role, StopReason, TokenUsage, Tool, ToolSpec,
};

use slimcode_sdk::{AgentSessionOptions, SdkEvent, create_agent_session};

/// The system message a session's provider saw on its first request (the
/// runner prepends it as the first message; ADR-0012 D3).
fn captured_system(seen: &[Vec<Message>]) -> String {
    seen.first()
        .expect("at least one provider call")
        .iter()
        .find(|m| m.role == Role::System)
        .expect("system message")
        .text_content()
}

/// Write a skill at `<home>/skills/<name>/SKILL.md` with the given body line.
fn write_user_skill(home: &Path, name: &str, description: &str, disable: bool, body: &str) {
    let dir = home.join("skills").join(name);
    fs::create_dir_all(&dir).expect("skill dir");
    let frontmatter = if disable {
        "disable-model-invocation: true\n"
    } else {
        ""
    };
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n{frontmatter}---\n\n{body}\n"),
    )
    .expect("write SKILL.md");
}

// --- AGENTS.md context-file discovery -------------------------------------

#[test]
fn agents_md_is_injected_into_the_system_prompt() {
    let (_guard, _home, cwd) = test_env("context-inject");
    fs::write(cwd.join("AGENTS.md"), "project rules\n").expect("write AGENTS.md");

    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider =
        FakeProvider::new(vec![vec![text("ok"), done_stop()]]).records_into(seen.clone());
    let session = make_session(&cwd, Box::new(provider), Vec::new());

    session.prompt("hi").expect("prompt");

    let system = captured_system(&seen.lock().expect("seen lock"));
    assert!(system.contains("## Project context"), "got: {system}");
    assert!(system.contains("<project_instructions"), "got: {system}");
    assert!(system.contains("scope=\"project\""), "got: {system}");
    // The file's content lands inside the block (trimmed like the CLI).
    assert!(system.contains("project rules"), "got: {system}");
    // The project block follows the `## Environment` section.
    let env = system.find("## Environment").unwrap();
    let ctx = system.find("## Project context").unwrap();
    assert!(env < ctx, "environment must precede project context");
}

#[test]
fn global_agents_md_from_the_slimcode_home_is_injected_too() {
    let (_guard, home, cwd) = test_env("context-global");
    // The global file lives at `<home>/AGENTS.md` (the slimcode home dir);
    // the project file at `<cwd>/AGENTS.md`. Both are discovered, global
    // first, and both land in the `## Project context` section.
    fs::write(home.join("AGENTS.md"), "global rules\n").expect("write global AGENTS.md");
    fs::write(cwd.join("AGENTS.md"), "project rules\n").expect("write project AGENTS.md");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider =
        FakeProvider::new(vec![vec![text("ok"), done_stop()]]).records_into(seen.clone());
    let session = make_session(&cwd, Box::new(provider), Vec::new());

    session.prompt("hi").expect("prompt");

    let system = captured_system(&seen.lock().expect("seen lock"));
    assert!(system.contains("## Project context"), "got: {system}");
    assert!(system.contains("scope=\"global\""), "got: {system}");
    assert!(system.contains("scope=\"project\""), "got: {system}");
    assert!(system.contains("global rules"), "got: {system}");
    assert!(system.contains("project rules"), "got: {system}");
    // Global comes first, then the project file.
    let global = system.find("global rules").unwrap();
    let project = system.find("project rules").unwrap();
    assert!(global < project, "global must precede project");
}

#[test]
fn no_agents_md_means_no_project_context_section_and_byte_identical_system() {
    let (_guard, home, cwd) = test_env("context-none");
    // No AGENTS.md anywhere: neither the temp home nor the cwd has one.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider =
        FakeProvider::new(vec![vec![text("ok"), done_stop()]]).records_into(seen.clone());
    let session = make_session(&cwd, Box::new(provider), Vec::new());

    session.prompt("hi").expect("prompt");

    let system = captured_system(&seen.lock().expect("seen lock"));
    assert!(!system.contains("## Project context"), "got: {system}");
    assert!(!system.contains("<project_instructions"), "got: {system}");
    // Byte-identical to the no-context-files assembly: the base prompt plus
    // exactly the `## Environment` section, nothing more (format_context_files
    // with an empty list contributes zero bytes).
    let expected = format!(
        "{DEFAULT_SYSTEM_PROMPT}\n\n## Environment\n\n- OS: {}\n- global home: {}\n- project home: {}\n",
        std::env::consts::OS,
        home.display(),
        cwd.display(),
    );
    assert_eq!(system, expected);
}

// --- skills discovery + read interception ---------------------------------

#[test]
fn skills_are_discovered_and_advertised_only_when_auto_invokable() {
    let (_guard, home, cwd) = test_env("skills-advertise");
    // User-scope skills under `<home>/skills/`: one auto-invokable, one
    // `disable-model-invocation: true`.
    write_user_skill(&home, "tdd", "test first", false, "red green refactor");
    write_user_skill(
        &home,
        "grill",
        "stress-test a plan",
        true,
        "interview the plan",
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider =
        FakeProvider::new(vec![vec![text("ok"), done_stop()]]).records_into(seen.clone());
    let session = make_session(&cwd, Box::new(provider), Vec::new());

    session.prompt("hi").expect("prompt");

    let system = captured_system(&seen.lock().expect("seen lock"));
    assert!(system.contains("## Skills"), "got: {system}");
    // The auto-invokable skill is advertised with its name, description and
    // the `SKILL.md` location the model can read.
    let skill_file = home.join("skills").join("tdd").join("SKILL.md");
    assert!(
        system.contains(&format!(
            "- tdd: test first [Read from {}]",
            skill_file.display()
        )),
        "got: {system}"
    );
    // The `disable-model-invocation` skill stays out of the prompt entirely.
    assert!(!system.contains("grill"), "got: {system}");
    assert!(!system.contains("stress-test a plan"), "got: {system}");
    // The skills index follows the environment / project context sections.
    let env = system.find("## Environment").unwrap();
    let skills_idx = system.find("## Skills").unwrap();
    assert!(env < skills_idx, "skills index must follow environment");
}

#[test]
fn model_read_of_a_skill_skill_md_is_intercepted_as_the_activation_block() {
    let (_guard, home, cwd) = test_env("skills-read");
    write_user_skill(&home, "tdd", "test first", false, "red green refactor");
    // The scripted model issues a `read` of the skill's own SKILL.md (as a
    // path relative to cwd), then finishes. The interception replaces the
    // read with the skill activation block; the tool never runs.
    let script = vec![
        vec![
            tc_start(0, "call_1", "read"),
            tc_args(0, r#"{"path": "../home/skills/tdd/SKILL.md"}"#),
            done_tools(),
        ],
        vec![text("done"), done_stop()],
    ];
    let session = make_session(&cwd, Box::new(FakeProvider::new(script)), Vec::new());
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    session.prompt("use tdd").expect("prompt");

    // The tool result the model would receive carries the activation block:
    // the `<skill name=...>` wrapper plus the skill body text.
    let tool_results: Vec<String> = recorder
        .events()
        .into_iter()
        .filter_map(|e| match e {
            SdkEvent::Message { message, .. } if message.role() == &Role::Tool => {
                Some(message.text_content())
            }
            _ => None,
        })
        .collect();
    assert_eq!(tool_results.len(), 1, "got: {tool_results:?}");
    let result = &tool_results[0];
    assert!(
        result.contains("<skill name=\"tdd\""),
        "activation block missing: {result}"
    );
    assert!(
        result.contains("References are relative to"),
        "got: {result}"
    );
    assert!(result.contains("red green refactor"), "got: {result}");
    assert!(result.ends_with("</skill>"), "got: {result}");
}

#[test]
fn already_loaded_skill_read_yields_the_notice_not_the_body() {
    let (_guard, home, cwd) = test_env("skills-already-loaded");
    write_user_skill(&home, "tdd", "test first", false, "red green refactor");
    // The user prompt itself already carries the skill's activation block, so
    // the dedup history marks the skill loaded; a later `read` of its SKILL.md
    // is served the already-loaded notice instead of the body again.
    let script = vec![
        vec![
            tc_start(0, "call_1", "read"),
            tc_args(0, r#"{"path": "../home/skills/tdd/SKILL.md"}"#),
            done_tools(),
        ],
        vec![text("done"), done_stop()],
    ];
    let session = make_session(&cwd, Box::new(FakeProvider::new(script)), Vec::new());
    let recorder = Recorder::new();
    let _unsubscribe = session.subscribe(recorder.listener());

    session
        .prompt("run <skill name=\"tdd\"> now")
        .expect("prompt");

    let tool_results: Vec<String> = recorder
        .events()
        .into_iter()
        .filter_map(|e| match e {
            SdkEvent::Message { message, .. } if message.role() == &Role::Tool => {
                Some(message.text_content())
            }
            _ => None,
        })
        .collect();
    assert_eq!(tool_results.len(), 1, "got: {tool_results:?}");
    let result = &tool_results[0];
    assert!(result.contains("<skill name=\"tdd\""), "got: {result}");
    // The already-loaded notice replaces the body.
    assert!(result.contains("already loaded"), "got: {result}");
    assert!(result.contains("not repeated here"), "got: {result}");
    assert!(!result.contains("red green refactor"), "got: {result}");
}

#[test]
fn project_scope_skills_are_discovered_and_override_user_on_name_clash() {
    let (_guard, home, cwd) = test_env("skills-project");
    // A user-scope skill and a project-scope skill under `<cwd>/.slimcode/`;
    // the project scope wins on a name clash (SkillStore semantics).
    write_user_skill(&home, "shared", "user version", false, "user body");
    let proj_dir = cwd.join(".slimcode").join("skills").join("shared");
    fs::create_dir_all(&proj_dir).expect("project skill dir");
    fs::write(
        proj_dir.join("SKILL.md"),
        "---\nname: shared\ndescription: project version\n---\n\nproject body\n",
    )
    .expect("write project SKILL.md");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider =
        FakeProvider::new(vec![vec![text("ok"), done_stop()]]).records_into(seen.clone());
    let session = make_session(&cwd, Box::new(provider), Vec::new());

    session.prompt("hi").expect("prompt");

    let system = captured_system(&seen.lock().expect("seen lock"));
    assert!(system.contains("## Skills"), "got: {system}");
    // The project version wins; its file location is advertised.
    let proj_file = proj_dir.join("SKILL.md");
    assert!(
        system.contains(&format!(
            "- shared: project version [Read from {}]",
            proj_file.display()
        )),
        "got: {system}"
    );
    assert!(!system.contains("user version"), "got: {system}");
}

// --- explicit config crossing the chat seam --------------------------------

/// What a provider saw on the `chat` seam: the resolved config (model, base
/// URL, cache flag, API key).
#[derive(Clone, Debug, PartialEq, Eq)]
struct SeenConfig {
    model: String,
    base_url: String,
    cache: bool,
    api_key: String,
}

/// A stateless provider that records exactly what the caller resolved and
/// reports it on the `chat` seam (the
/// `provider_receives_the_config_crossing_the_chat_seam` precedent).
struct CapturingProvider {
    seen: Arc<Mutex<Option<SeenConfig>>>,
}

impl CapturingProvider {
    fn boxed(seen: &Arc<Mutex<Option<SeenConfig>>>) -> Box<dyn Provider + Send> {
        Box::new(Self {
            seen: Arc::clone(seen),
        })
    }
}

impl Provider for CapturingProvider {
    fn chat(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        config: &ProviderConfig,
        _cancel: &CancelToken,
        on_delta: &mut dyn FnMut(slimcode_app::Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        *self.seen.lock().expect("seen lock") = Some(SeenConfig {
            model: config.model.clone(),
            base_url: config.base_url.clone(),
            cache: config.cache,
            api_key: config.api_key.clone(),
        });
        on_delta(text("ok"))?;
        on_delta(done_stop())?;
        Ok(None)
    }
}

#[test]
fn explicit_config_crosses_the_chat_seam_verbatim() {
    let (_guard, _home, cwd) = test_env("explicit-config");
    // A stateless provider records exactly what the caller resolved (the
    // `provider_receives_the_config_crossing_the_chat_seam` precedent): the
    // explicit options must arrive across the seam unmodified.
    let seen = Arc::new(Mutex::new(None::<SeenConfig>));
    let session = create_agent_session(
        AgentSessionOptions::new(Some(cwd))
            .with_api_key("sk-explicit")
            .with_base_url("https://explicit.example.com/v1")
            .with_model("explicit-model")
            .with_cache(false)
            .with_provider(CapturingProvider::boxed(&seen))
            .with_tools(Vec::new()),
    )
    .expect("create_agent_session");

    session.prompt("hi").expect("prompt");

    assert_eq!(
        *seen.lock().expect("seen lock"),
        Some(SeenConfig {
            model: "explicit-model".to_string(),
            base_url: "https://explicit.example.com/v1".to_string(),
            cache: false,
            api_key: "sk-explicit".to_string(),
        }),
    );
}

#[test]
fn explicit_config_overrides_the_config_file_layer() {
    let (_guard, home, cwd) = test_env("explicit-config-precedence");
    // `config.toml` in the slimcode home is the file layer of the four-layer
    // resolution (overrides > env > config.toml > defaults). Explicit options
    // must win over it, so an embedder can point at a self-hosted endpoint
    // without editing — or even having — the user's config file.
    fs::write(
        home.join("config.toml"),
        "[ai]\n\
         base_url = \"https://file.example.com/v1\"\n\
         model = \"file-model\"\n\
         cache = true\n\
         api_key = \"sk-file\"\n",
    )
    .expect("write config.toml");
    let seen = Arc::new(Mutex::new(None::<SeenConfig>));
    let session = create_agent_session(
        AgentSessionOptions::new(Some(cwd))
            .with_api_key("sk-explicit")
            .with_base_url("https://explicit.example.com/v1")
            .with_model("explicit-model")
            .with_cache(false)
            .with_provider(CapturingProvider::boxed(&seen))
            .with_tools(Vec::new()),
    )
    .expect("create_agent_session");

    session.prompt("hi").expect("prompt");

    assert_eq!(
        *seen.lock().expect("seen lock"),
        Some(SeenConfig {
            model: "explicit-model".to_string(),
            base_url: "https://explicit.example.com/v1".to_string(),
            cache: false,
            api_key: "sk-explicit".to_string(),
        }),
    );
}

// --- injected provider + custom tool take effect ---------------------------

#[test]
fn injected_custom_tool_executes_with_its_side_effect() {
    let (_guard, _home, cwd) = test_env("custom-tool");
    let marker = cwd.join("marker.txt");
    let marker_path = marker.clone();
    let mark_tool = Tool::new(
        "mark",
        "writes a marker file",
        serde_json::json!({}),
        move |_| {
            fs::write(&marker_path, "ran").map_err(|e| e.to_string())?;
            Ok("marked".to_string())
        },
    );
    // The scripted model calls the injected tool, then finishes.
    let script = vec![
        vec![
            tc_start(0, "call_1", "mark"),
            tc_args(0, "{}"),
            done_tools(),
        ],
        vec![text("done"), done_stop()],
    ];
    let session = make_session(&cwd, Box::new(FakeProvider::new(script)), vec![mark_tool]);

    let stop = session.prompt("mark it").expect("prompt");
    assert_eq!(stop, StopReason::Completed);
    // The custom tool ran: its side effect is observable after the turn.
    assert!(marker.exists(), "marker file missing");
    assert_eq!(fs::read_to_string(&marker).expect("read marker"), "ran");
}
