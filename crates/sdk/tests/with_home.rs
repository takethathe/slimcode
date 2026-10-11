//! `with_home` pins the slimcode home (ticket 01): the session store root,
//! the skills / context-file discovery and `Environment.global_home` all land
//! under the pinned directory — even when a different `SLIMCODE_HOME` env var
//! is set (the process-global source a host cannot control from inside a
//! test). Driven through the public `AgentSession` API only, per the spec's
//! Testing Decisions.

mod common;
use common::*;

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use slimcode_app::{Message, Role};

use slimcode_sdk::{AgentSessionOptions, create_agent_session};

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
fn write_user_skill(home: &Path, name: &str, description: &str, body: &str) {
    let dir = home.join("skills").join(name);
    fs::create_dir_all(&dir).expect("skill dir");
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )
    .expect("write SKILL.md");
}

/// `with_home(pin)` wins over a distracting `SLIMCODE_HOME` env var: the
/// session persists under the pinned home (the interfering home stays
/// untouched), and skills / global AGENTS.md discovery plus
/// `Environment.global_home` all resolve to the pinned directory.
#[test]
fn with_home_pins_the_session_store_and_discovery_against_the_env() {
    let (pinned, cwd) = scratch("with-home");
    // The distracting env: `SLIMCODE_HOME` points at a sibling directory that
    // must NOT be touched by the session.
    let interfering = pinned.parent().expect("scratch root").join("interfering");
    fs::create_dir_all(&interfering).expect("interfering");
    let _guard = home_guard(&interfering);

    // Discovery sources under the pinned home…
    write_user_skill(&pinned, "tdd", "test first", "red green refactor");
    fs::write(pinned.join("AGENTS.md"), "pinned global rules\n").expect("write pinned AGENTS.md");
    // …and decoys under the interfering home that must not be discovered.
    write_user_skill(&interfering, "decoy", "do not advertise", "decoy body");
    fs::write(interfering.join("AGENTS.md"), "interfering global rules\n")
        .expect("write interfering AGENTS.md");

    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider =
        FakeProvider::new(vec![vec![text("ok"), done_stop()]]).records_into(seen.clone());
    let session = create_agent_session(
        AgentSessionOptions::new(Some(cwd.clone()))
            .with_api_key("sk-test")
            .with_base_url("https://example.invalid/v1")
            .with_model("test-model")
            .with_home(pinned.clone())
            .with_provider(Box::new(provider)),
    )
    .expect("create_agent_session");

    session.prompt("hi").expect("prompt");

    let system = captured_system(&seen.lock().expect("seen lock"));
    // `Environment.global_home` is the pinned home.
    assert!(
        system.contains(&format!("- global home: {}", pinned.display())),
        "got: {system}"
    );
    // The pinned skill is advertised with its own path; the decoy is not.
    let skill_file = pinned.join("skills").join("tdd").join("SKILL.md");
    assert!(
        system.contains(&format!(
            "- tdd: test first [Read from {}]",
            skill_file.display()
        )),
        "got: {system}"
    );
    assert!(!system.contains("decoy"), "got: {system}");
    // The pinned global AGENTS.md enters the context; the interfering one does
    // not.
    assert!(system.contains("pinned global rules"), "got: {system}");
    assert!(
        !system.contains("interfering global rules"),
        "got: {system}"
    );
    // The session log landed under the pinned home; the interfering home's
    // sessions directory was never even created.
    assert_eq!(count_jsonl(&pinned.join("sessions")), 1, "pinned store");
    assert!(
        !interfering.join("sessions").exists(),
        "the interfering home must stay untouched"
    );
}
