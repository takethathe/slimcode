//! SDK session resume (`AgentSessionOptions.with_session_id`): a session can
//! be recreated from a previously persisted session id — replaying its
//! messages and title, continuing the same JSONL log, and starting usage at
//! zero (usage is never persisted, ADR-0018 D4). Loading requires a store, so
//! `persist: false` is rejected with a clear error, and an unknown id errors
//! loudly — driven through the public `AgentSession` API only (spec Testing
//! Decisions).

mod common;
use common::*;

use std::path::Path;

use slimcode_sdk::{AgentSession, AgentSessionOptions, Provider, create_agent_session};

/// Build a resumed session through the public API: the same explicit config
/// `make_session` uses (env independence), plus the session id to resume.
fn resume_session(cwd: &Path, provider: Box<dyn Provider + Send>, id: &str) -> AgentSession {
    create_agent_session(
        AgentSessionOptions::new(Some(cwd.to_path_buf()))
            .with_api_key("sk-test")
            .with_base_url("https://example.invalid/v1")
            .with_model("test-model")
            .with_provider(provider)
            .with_session_id(id.to_string()),
    )
    .expect("create_agent_session")
}

// --- behavior -------------------------------------------------------------

/// A persisted session survives disposal: recreating it by id yields the same
/// id, the same messages and title, and a zeroed usage; the resumed session
/// keeps appending to the same log, so a replay sees the appended view.
#[test]
fn resume_replays_messages_and_title_and_continues_the_same_log() {
    let (_guard, home, cwd) = test_env("resume");
    // Session A: two prompts through the default (persist: true) path.
    let a = make_session(
        &cwd,
        Box::new(FakeProvider::new(vec![
            vec![text("reply one"), done_stop()],
            vec![text("reply two"), done_stop()],
        ])),
        Vec::new(),
    );
    a.prompt("first").expect("first prompt");
    a.prompt("second").expect("second prompt");
    let id = a.session_id().to_string();
    let a_messages = a.messages();
    let a_title = a.title();
    assert!(a_title.is_some(), "first prompts infer a title");
    assert_eq!(a_messages.len(), 4);
    a.dispose();

    // Session B resumes A by id: same identity, messages, and title; usage
    // starts from zero (usage is in-memory only, ADR-0018 D4).
    let b = resume_session(
        &cwd,
        Box::new(FakeProvider::new(vec![vec![
            text("reply three"),
            done_stop(),
        ]])),
        &id,
    );
    assert_eq!(b.session_id(), id);
    assert_eq!(b.messages(), a_messages);
    assert_eq!(b.title(), a_title);
    assert_eq!(b.usage().total_tokens, 0);

    // The resumed session continues the same log: a third prompt appends to
    // the same file (no second log), and a replay yields the appended view
    // with the new turn in it.
    b.prompt("third").expect("third prompt");
    assert_eq!(b.messages().len(), 6); // three user/assistant pairs
    assert_eq!(count_jsonl(&home.join("sessions")), 1);
    let store = store_for(&home, &cwd);
    let outcome = store.load(&id).expect("load");
    assert_eq!(outcome.session.messages, b.messages());
    let texts: Vec<String> = outcome
        .session
        .messages
        .iter()
        .map(|m| m.text_content().to_string())
        .collect();
    assert!(texts.contains(&"third".to_string()), "{texts:?}");
    assert_eq!(outcome.skipped_records, 0);
}

/// Resuming loads from the session store, so `persist: false` (no store) is
/// rejected up front with a clear error instead of a silent no-op.
#[test]
fn resume_with_persist_false_errors() {
    let (_guard, _home, cwd) = test_env("resume-nopersist");
    let err = create_agent_session(
        AgentSessionOptions::new(Some(cwd))
            .with_api_key("sk-test")
            .with_base_url("https://example.invalid/v1")
            .with_model("test-model")
            .with_provider(Box::new(FakeProvider::new(Vec::new())))
            .with_persist(false)
            .with_session_id("slimcode-0-0-0".to_string()),
    )
    .err()
    .expect("expected a resume error");
    assert!(err.contains("persist"), "err: {err}");
}

/// An unknown session id is a loud creation error naming the id, never a
/// silently fresh session.
#[test]
fn resume_with_unknown_id_errors() {
    let (_guard, _home, cwd) = test_env("resume-unknown");
    let err = create_agent_session(
        AgentSessionOptions::new(Some(cwd))
            .with_api_key("sk-test")
            .with_base_url("https://example.invalid/v1")
            .with_model("test-model")
            .with_provider(Box::new(FakeProvider::new(Vec::new())))
            .with_session_id("slimcode-0-0-0".to_string()),
    )
    .err()
    .expect("expected a resume error");
    assert!(err.contains("cannot resume session"), "err: {err}");
    assert!(err.contains("slimcode-0-0-0"), "err: {err}");
}
