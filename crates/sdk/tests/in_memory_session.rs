//! The SDK's pure in-memory session mode
//! (`persist: false`), driven through the public `AgentSession` API only
//! (spec Testing Decisions items 4-5).
//!
//! A scripted `FakeProvider` injected via `AgentSessionOptions.provider` runs
//! against the real shared turn loop; assertions cover the zero-file
//! guarantee of memory mode, the event-sequence equivalence with persistent
//! mode, in-memory messages / usage / title accumulation across turns, and
//! the shared session-id scheme (ticket 01).

mod common;
use common::*;

use std::fs;
use std::path::{Path, PathBuf};

use slimcode_app::render::DisplayItem;
use slimcode_app::testutil::unique_temp_dir;
use slimcode_app::{Delta, StopReason};

use slimcode_sdk::{AgentSession, AgentSessionOptions, SdkEvent, create_agent_session};

/// A fresh `root` (everything the turn may write must stay under it), with a
/// `home` (the `SLIMCODE_HOME` target) and a `cwd` (a git repo, so the
/// project home resolves deterministically to `cwd` itself) inside.
fn scratch_root(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = unique_temp_dir(&format!("slimcode-sdk-{tag}"));
    fs::create_dir_all(&root).expect("root");
    let home = root.join("home");
    let cwd = root.join("proj");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&cwd).expect("cwd");
    fs::create_dir_all(cwd.join(".git")).expect("git marker");
    (root, home, cwd)
}

/// Recursively count the files under `dir` (directories themselves never
/// count, so the pre-existing scratch layout contributes zero).
fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let ty = entry.file_type().expect("file type");
        n += if ty.is_dir() {
            count_files(&entry.path())
        } else {
            1
        };
    }
    n
}

/// Build a session through the public API: explicit config to avoid env
/// dependence, cwd under the test's temp dir, `persist` choosing the mode,
/// and the injected provider/tools.
fn make_session(
    cwd: &Path,
    persist: bool,
    provider: Box<dyn slimcode_app::Provider + Send>,
    tools: Vec<slimcode_app::Tool>,
) -> AgentSession {
    create_agent_session(
        AgentSessionOptions::new(Some(cwd.to_path_buf()))
            .with_persist(persist)
            .with_api_key("sk-test")
            .with_base_url("https://example.invalid/v1")
            .with_model("test-model")
            .with_provider(provider)
            .with_tools(tools),
    )
    .expect("create_agent_session")
}

/// Both modes must deliver the exact same event stream for the same prompt:
/// same kinds, same order, same payloads (the scripted provider's usage is
/// deterministic, so nothing may differ).
fn assert_event_streams_equal(memory: Vec<SdkEvent>, persistent: Vec<SdkEvent>) {
    assert_eq!(
        memory.len(),
        persistent.len(),
        "event counts differ: memory {} vs persistent {}",
        memory.len(),
        persistent.len()
    );
    for (m, p) in memory.iter().zip(persistent.iter()) {
        match (m, p) {
            (SdkEvent::Display(a), SdkEvent::Display(b)) => {
                assert_eq!(a, b, "display payload differs")
            }
            (
                SdkEvent::Message {
                    message: am,
                    usage: au,
                },
                SdkEvent::Message {
                    message: bm,
                    usage: bu,
                },
            ) => {
                assert_eq!(am, bm, "message differs");
                assert_eq!(au, bu, "message usage differs");
            }
            (
                SdkEvent::TurnFinished { stop_reason: a },
                SdkEvent::TurnFinished { stop_reason: b },
            ) => assert_eq!(a, b, "stop reason differs"),
            (SdkEvent::Compacted { .. }, SdkEvent::Compacted { .. }) => {}
            (a, b) => panic!("event kinds differ: {a:?} vs {b:?}"),
        }
    }
}

/// The tool-calling script used by the mode-equivalence tests: two provider
/// calls (assistant-with-tool-calls → tool result → final assistant), so the
/// stream exercises Turn/Text/ToolStart/ToolResult/Usage displays, per-message
/// `Message` events (usage on assistants, `None` on the user prompt and the
/// tool result) and the final `TurnFinished`.
fn tool_script() -> Vec<Vec<Delta>> {
    vec![
        vec![
            text("checking "),
            tc_start(0, "call_1", "probe"),
            tc_args(0, "{}"),
            done_tools(),
        ],
        vec![text("probed!"), done_stop()],
    ]
}

// --- behavior -------------------------------------------------------------

#[test]
fn memory_mode_turn_writes_no_files_anywhere() {
    let (root, home, cwd) = scratch_root("mem-nofiles");
    let _guard = home_guard(&home);
    let provider = FakeProvider::new(tool_script()).with_per_call(usage(10, 5));
    let session = make_session(&cwd, false, Box::new(provider), vec![probe_tool()]);

    let stop = session.prompt("go").expect("prompt");
    assert_eq!(stop, StopReason::Completed);

    // The turn is complete; nothing may have hit disk under the temp root:
    // no session store, no `.jsonl`, no files of any kind.
    assert!(
        !home.join("sessions").exists(),
        "memory mode must not create a sessions store"
    );
    let files = count_files(&root);
    assert_eq!(
        files,
        0,
        "memory mode must not write files; found {files} under {}",
        root.display()
    );
}

#[test]
fn memory_and_persistent_modes_emit_identical_event_sequences() {
    let (root, home, cwd) = scratch_root("mem-vs-persist");
    let _guard = home_guard(&home);
    let per_call = usage(10, 5);

    let memory = make_session(
        &cwd,
        false,
        Box::new(FakeProvider::new(tool_script()).with_per_call(per_call)),
        vec![probe_tool()],
    );
    let persistent = make_session(
        &cwd,
        true,
        Box::new(FakeProvider::new(tool_script()).with_per_call(per_call)),
        vec![probe_tool()],
    );
    let mem_rec = Recorder::new();
    let per_rec = Recorder::new();
    let _m = memory.subscribe(mem_rec.listener());
    let _p = persistent.subscribe(per_rec.listener());

    assert_eq!(
        memory.prompt("go").expect("memory prompt"),
        StopReason::Completed
    );
    assert_eq!(
        persistent.prompt("go").expect("persistent prompt"),
        StopReason::Completed
    );

    // Identical event streams and identical session state — only the
    // persistence point differs.
    assert_event_streams_equal(mem_rec.events(), per_rec.events());
    assert_eq!(memory.messages(), persistent.messages());
    assert_eq!(memory.usage(), persistent.usage());
    assert_eq!(memory.title(), persistent.title());

    // The persistent session wrote its log; the memory session still wrote
    // nothing (proves the sequences really came from two distinct modes).
    assert!(count_files(&root) > 0, "persistent mode must write a log");
    assert_eq!(count_files(&root.join("home").join("sessions")), 1);
}

#[test]
fn memory_mode_accumulates_messages_usage_and_title_across_prompts() {
    let (root, home, cwd) = scratch_root("mem-multi");
    let _guard = home_guard(&home);
    let per_call = usage(10, 5);
    let provider = FakeProvider::new(vec![
        vec![text("reply one"), done_stop()],
        vec![text("reply two"), done_stop()],
    ])
    .with_per_call(per_call);
    let session = make_session(&cwd, false, Box::new(provider), Vec::new());
    let recorder = Recorder::new();
    let _u = session.subscribe(recorder.listener());

    session.prompt("first").expect("first");
    session.prompt("second").expect("second");

    // History accumulates in memory: user1, asst1, user2, asst2.
    let messages = session.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].text_content(), "first");
    assert_eq!(messages[1].text_content(), "reply one");
    assert_eq!(messages[2].text_content(), "second");
    assert_eq!(messages[3].text_content(), "reply two");

    // Usage accumulates across turns: one assistant usage per turn.
    assert_eq!(session.usage(), usage(20, 10));

    // The title is inferred from the first prompt and kept.
    assert_eq!(session.title().as_deref(), Some("first"));

    // Each prompt produced exactly one authoritative end signal.
    let finished = recorder
        .events()
        .into_iter()
        .filter(|e| matches!(e, SdkEvent::TurnFinished { .. }))
        .count();
    assert_eq!(finished, 2);

    // Two turns in memory mode still leave the temp root file-free.
    assert_eq!(
        count_files(&root),
        0,
        "memory mode must not write files, found {} under {}",
        count_files(&root),
        root.display()
    );
}

#[test]
fn memory_mode_ids_use_the_shared_scheme_and_are_unique() {
    let (root, home, cwd) = scratch_root("mem-ids");
    let _guard = home_guard(&home);
    let a = make_session(
        &cwd,
        false,
        Box::new(FakeProvider::new(Vec::new())),
        Vec::new(),
    );
    let b = make_session(
        &cwd,
        false,
        Box::new(FakeProvider::new(Vec::new())),
        Vec::new(),
    );

    let id_a = a.session_id();
    let id_b = b.session_id();
    // The shared `slimcode-<unix>-<pid>-<n>` scheme (ticket 01) — the same
    // shape a persistent session gets — and distinct per session.
    for id in [id_a, id_b] {
        assert!(id.starts_with("slimcode-"), "id: {id}");
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 4, "id shape: {id}");
        for part in &parts[1..] {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "id component not numeric: {id}"
            );
        }
    }
    assert_ne!(id_a, id_b, "two sessions must not share an id");
    assert_eq!(count_files(&root), 0);
}

/// Checklist item 1's tool-turn clause: a turn that emits reasoning and calls
/// a tool streams the ordered display sequence
/// Turn / Reasoning / Text / ToolStart / ToolResult / Stop, and the tool
/// really ran.
#[test]
fn tool_turn_streams_the_ordered_display_sequence() {
    let (_root, home, cwd) = scratch_root("tool-displays");
    let _guard = home_guard(&home);
    let provider = FakeProvider::new(vec![
        vec![
            Delta::Reasoning("weighing the request ".into()),
            text("checking "),
            tc_start(0, "call_1", "probe"),
            tc_args(0, "{}"),
            done_tools(),
        ],
        vec![text("probed!"), done_stop()],
    ]);
    let session = make_session(&cwd, true, Box::new(provider), vec![probe_tool()]);
    let recorder = Recorder::new();
    let _u = session.subscribe(recorder.listener());

    assert_eq!(session.prompt("go").expect("prompt"), StopReason::Completed);

    let displays = recorder.displays();
    // The turn opens on its marker and the reasoning delta is displayed.
    assert_eq!(displays.first(), Some(&DisplayItem::Turn { turn: 1 }));
    let reasoning = displays
        .iter()
        .position(|d| matches!(d, DisplayItem::Reasoning(_)))
        .expect("the reasoning delta must be displayed");

    // The text fragments arrive in script order (one per provider call).
    let texts: Vec<&str> = displays
        .iter()
        .filter_map(|d| match d {
            DisplayItem::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["checking ", "probed!"]);

    // The tool start precedes its matching result: same id, the tool's name,
    // the aggregated arguments, and the tool actually ran (ok).
    let start = displays
        .iter()
        .position(|d| matches!(d, DisplayItem::ToolStart { .. }))
        .expect("the tool start must be displayed");
    let result = displays
        .iter()
        .position(|d| matches!(d, DisplayItem::ToolResult { .. }))
        .expect("the tool result must be displayed");
    assert!(reasoning < start, "reasoning must precede the tool call");
    assert!(start < result, "ToolStart must precede ToolResult");
    match &displays[start] {
        DisplayItem::ToolStart {
            tool_call_id,
            name,
            arguments,
        } => {
            assert_eq!(tool_call_id, "call_1");
            assert_eq!(name, "probe");
            assert_eq!(arguments, "{}");
        }
        other => panic!("expected ToolStart, got {other:?}"),
    }
    match &displays[result] {
        DisplayItem::ToolResult {
            tool_call_id,
            name,
            ok,
            ..
        } => {
            assert_eq!(tool_call_id, "call_1");
            assert_eq!(name, "probe");
            assert!(*ok, "the probe tool succeeded");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }

    // The whole turn ends on the single authoritative stop marker.
    assert_eq!(
        displays.last(),
        Some(&DisplayItem::Stop(StopReason::Completed))
    );
}
