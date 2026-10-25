//! Equivalence golden baseline (`.scratch/engine` ticket 01, spec Testing
//! Decisions items 13–18).
//!
//! This is the migration's acceptance gate: it pins the *public observable
//! behavior* of today's `AgentSession` (event order, `StopReason`, the
//! persisted JSONL log, usage, title, turn-end compaction's two paths, and the
//! skill-read interception text) against fixed golden values, produced through
//! the public SDK API only with scripted providers. Every later engine ticket
//! (the seven-phase rewire) must keep these green — a drift here is a
//! regression, not a refactor. The harness is the reusable "same scripted
//! inputs → expected external output" comparison entry the tickets reference.
//!
//! Everything environment-dependent (the per-run session id, RFC3339
//! timestamps, the temp home/cwd absolute paths baked into the header and the
//! skill `location`/base-dir lines) is masked to stable placeholders, so the
//! remaining bytes / event strings are deterministic across runs and across
//! the migration.

mod common;
use common::*;

use std::path::Path;
use std::sync::{Arc, Mutex};

use slimcode_app::render::DisplayItem;
use slimcode_app::skills::SkillStore;
use slimcode_app::{AgentMessage, CancelToken};

use slimcode_sdk::{AgentSession, AgentSessionOptions, SdkEvent, create_agent_session};

/// One turn's whole external footprint, normalized for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observable {
    /// The ordered event stream (every `SdkEvent`, one compact string each).
    events: Vec<String>,
    /// The `prompt` return value.
    stop: Option<String>,
    /// The error string when `prompt` failed (one line, temp paths masked).
    error: Option<String>,
    /// The session title after the turn.
    title: Option<String>,
    /// Accumulated total tokens.
    usage_total: u64,
    /// History as `{role}::{text}` (one entry per message that entered memory).
    history: Vec<String>,
    /// The persisted JSONL log, header line masked, one entry per line.
    log: Vec<String>,
}

/// Mask the environment-dependent substrings out of `s`.
fn mask(s: &str, home: &Path, cwd: &Path, sid: &str, created: &str) -> String {
    let mut out = s.to_string();
    // Longest / most specific first so a path inside a path is not partially
    // replaced.
    for (needle, tag) in [
        (cwd.to_string_lossy().as_ref(), "<CWD>"),
        (home.to_string_lossy().as_ref(), "<HOME>"),
        (sid, "<SESSION_ID>"),
        (created, "<CREATED_AT>"),
    ] {
        if !needle.is_empty() {
            out = out.replace(needle, tag);
        }
    }
    collapse_runs(&out)
}

/// Collapse any run of >= 20 identical characters to `<c x LEN>`, so the
/// compaction scenario's deliberately huge prompt (a means to cross the token
/// threshold) does not bloat the golden while staying deterministic.
fn collapse_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let mut run = 1usize;
        while chars.peek() == Some(&c) {
            run += 1;
            chars.next();
        }
        if run >= 20 {
            out.push_str(&format!("<{c} x {run}>"));
        } else {
            for _ in 0..run {
                out.push(c);
            }
        }
    }
    out
}

/// Normalize one history message to `{role}::{text}` (plus a tool-call tail so
/// an assistant turn's tool batch is visible).
fn norm_message(m: &AgentMessage, home: &Path, cwd: &Path, sid: &str, created: &str) -> String {
    let mut line = format!("{}::{}", label_role(m), m.text_content());
    let calls: Vec<String> = m
        .tool_calls()
        .iter()
        .map(|tc| format!("{}:{}", tc.id, tc.name))
        .collect();
    if !calls.is_empty() {
        line.push_str(&format!(" [{}]", calls.join(",")));
    }
    if let Some(id) = m.tool_call_id() {
        line.push_str(&format!(" (for {id})"));
    }
    mask(&line, home, cwd, sid, created)
}

fn label_role(m: &AgentMessage) -> &'static str {
    use slimcode_app::Role::*;
    match m.role() {
        System => "system",
        User => "user",
        Assistant => "assistant",
        Tool => "tool",
    }
}

/// Normalize one display item to a compact string.
fn norm_display(d: &DisplayItem) -> String {
    match d {
        DisplayItem::Turn { turn } => format!("disp:turn({turn})"),
        DisplayItem::Reasoning(t) => format!("disp:reasoning({t})"),
        DisplayItem::Text(t) => format!("disp:text({t})"),
        DisplayItem::ToolStart {
            tool_call_id,
            name,
            arguments,
        } => format!("disp:toolstart({tool_call_id},{name},{arguments})"),
        DisplayItem::ToolResult {
            tool_call_id,
            name,
            ok,
            result,
        } => format!("disp:toolresult({tool_call_id},{name},ok={ok},{result})"),
        DisplayItem::Stop(reason) => format!("disp:stop({reason:?})"),
        DisplayItem::Usage(u) => format!("disp:usage({})", u.total_tokens),
    }
}

/// Normalize one `SdkEvent`.
fn norm_event(e: &SdkEvent) -> String {
    match e {
        SdkEvent::Display(item) => norm_display(item),
        SdkEvent::Message { message, usage } => format!(
            "message({}::{};usage={})",
            label_role(message),
            message.text_content(),
            usage
                .map(|u| u.total_tokens.to_string())
                .unwrap_or("-".into())
        ),
        SdkEvent::TurnFinished { stop_reason } => format!("turn_finished({stop_reason:?})"),
        SdkEvent::Compacted { before, after } => format!("compacted({before},{after})"),
    }
}

/// Read the session's log, masking the header's identity fields; returns the
/// lines (empty when no log was written).
fn read_log(home: &Path, cwd: &Path, sid: &str, created: &str) -> Vec<String> {
    let store = store_for(home, cwd);
    let path = match store.session_path(sid) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                // Header: id / created_at / project_home are per-run; collapse
                // them to a fixed marker so the tail records are what is
                // compared.
                "<HEADER>".to_string()
            } else {
                mask(line, home, cwd, sid, created)
            }
        })
        .collect()
}

/// Run one prompt through a freshly-created session, capture its footprint.
/// `make` builds the (persisted) session in `env`'s cwd; the recorder captures
/// every event.
fn capture(
    home: &Path,
    cwd: &Path,
    provider: Box<dyn slimcode_app::Provider + Send>,
    tools: Vec<slimcode_app::Tool>,
    skills: Option<Vec<slimcode_app::skills::Skill>>,
    persist: bool,
    prompts: &[&str],
) -> Observable {
    let mut opts = AgentSessionOptions::new(Some(cwd.to_path_buf()))
        .with_api_key("sk-test")
        .with_base_url("https://example.invalid/v1")
        .with_model("test-model")
        .with_provider(provider)
        .with_tools(tools)
        .with_persist(persist);
    if let Some(skills) = skills {
        opts = opts.with_skills(skills);
    }
    let session = create_agent_session(opts).expect("create_agent_session");
    let sid = session.session_id().to_string();
    // The created_at is fixed by the store clock at creation; read it back from
    // the header we are about to mask so masking is exact.
    let created = created_for(&session, home, cwd);
    let recorder = Recorder::new();
    let _un = session.subscribe(recorder.listener());

    let mut stop: Option<String> = None;
    let mut error: Option<String> = None;
    for text in prompts {
        match session.prompt(text) {
            Ok(s) => stop = Some(format!("{s:?}")),
            Err(e) => {
                error = Some(mask(&e, home, cwd, &sid, &created));
                break;
            }
        }
    }

    let events: Vec<String> = recorder
        .events()
        .iter()
        .map(norm_event)
        .map(|e| mask(&e, home, cwd, &sid, &created))
        .collect();
    let history: Vec<String> = session
        .messages()
        .iter()
        .map(|m| norm_message(m, home, cwd, &sid, &created))
        .collect();
    let log = read_log(home, cwd, &sid, &created);

    Observable {
        events,
        stop,
        error,
        title: session.title().map(|t| mask(&t, home, cwd, &sid, &created)),
        usage_total: session.usage().total_tokens,
        history,
        log,
    }
}

/// The session's `created_at` as it appears in the persisted header (so masking
/// matches the exact string the run stamped).
fn created_for(session: &AgentSession, home: &Path, cwd: &Path) -> String {
    let store = store_for(home, cwd);
    let Ok(path) = store.session_path(session.session_id()) else {
        return String::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    // Pull the created_at value out of the header JSON line.
    match text.lines().next() {
        Some(line) => {
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            v.get("created_at")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string()
        }
        None => String::new(),
    }
}

/// A provider that treats a no-tools `chat` as the compaction summary request
/// and streams `summary_reply` for it; turn calls consume `script` in order.
/// `total` mirrors the default turn-end component's usage-delta accounting.
struct CompactProvider {
    script: Vec<Vec<slimcode_app::Delta>>,
    turn_calls: usize,
    summary_reply: String,
    fail_summary: bool,
    per_call: slimcode_app::TokenUsage,
    total: Arc<Mutex<slimcode_app::TokenUsage>>,
}

impl CompactProvider {
    fn new(script: Vec<Vec<slimcode_app::Delta>>, summary_reply: &str) -> Self {
        Self {
            script,
            turn_calls: 0,
            summary_reply: summary_reply.to_string(),
            fail_summary: false,
            per_call: slimcode_app::TokenUsage::default(),
            total: Arc::new(Mutex::new(slimcode_app::TokenUsage::default())),
        }
    }
    fn fails_summary(mut self) -> Self {
        self.fail_summary = true;
        self
    }
    fn with_per_call(mut self, usage: slimcode_app::TokenUsage) -> Self {
        self.per_call = usage;
        self
    }
}

impl slimcode_app::Provider for CompactProvider {
    fn chat(
        &mut self,
        _messages: &[slimcode_app::Message],
        tools: &[slimcode_app::ToolSpec],
        _config: &slimcode_app::ProviderConfig,
        _cancel: &CancelToken,
        on_delta: &mut dyn FnMut(slimcode_app::Delta) -> Result<(), String>,
    ) -> Result<Option<slimcode_app::TokenUsage>, String> {
        // A no-tools call is the compaction summary request.
        if tools.is_empty() {
            if self.fail_summary {
                return Err("summary exploded".to_string());
            }
            on_delta(text(&self.summary_reply))?;
            on_delta(done_stop())?;
            let mut t = self.total.lock().unwrap();
            *t = t.saturating_add(&self.per_call);
            return Ok(Some(self.per_call));
        }
        let batch = self
            .script
            .get(self.turn_calls)
            .cloned()
            .unwrap_or_default();
        self.turn_calls += 1;
        for d in batch {
            on_delta(d)?;
        }
        let mut t = self.total.lock().unwrap();
        *t = t.saturating_add(&self.per_call);
        Ok(Some(self.per_call))
    }
    fn total_usage(&self) -> slimcode_app::TokenUsage {
        *self.total.lock().unwrap()
    }
}

// --- scenarios -------------------------------------------------------------

/// A prompt that makes the model call a tool then answer: pins the full ordered
/// event stream, history, log, usage and title (spec items 13, 14, 15).
fn scenario_tool_turn(home: &Path, cwd: &Path) -> Observable {
    let provider = FakeProvider::new(vec![
        vec![
            text("checking "),
            tc_start(0, "call_1", "probe"),
            tc_args(0, "{}"),
            done_tools(),
        ],
        vec![text("done"), done_stop()],
    ])
    .with_per_call(usage(10, 5));
    capture(
        home,
        cwd,
        Box::new(provider),
        vec![probe_tool()],
        None,
        true,
        &["weather please"],
    )
}

/// A provider error on the second turn (after a first turn created the log):
/// pins the `Error`-reason closing message and the error'd log tail (spec
/// item 17).
fn scenario_error(home: &Path, cwd: &Path) -> Observable {
    // First turn succeeds and creates the log; the second call errors, so the
    // failed turn closes the existing log on an assistant `error` boundary.
    let provider = FakeProvider::new(vec![vec![text("first"), done_stop()], vec![]]).fails_on(1);
    capture(
        home,
        cwd,
        Box::new(provider),
        vec![probe_tool()],
        None,
        true,
        &["hello", "boom"],
    )
}

/// A cancel landing during the second request: pins the `Cancelled` close and
/// the "partial text never enters history" rule (spec item 17 / 8).
fn scenario_cancel(home: &Path, cwd: &Path) -> Observable {
    let provider = FakeProvider::new(vec![
        vec![
            tc_start(0, "call_1", "probe"),
            tc_args(0, "{}"),
            done_tools(),
        ],
        vec![text("final"), done_stop()],
    ])
    .cancels_on(2);
    capture(
        home,
        cwd,
        Box::new(provider),
        vec![probe_tool()],
        None,
        true,
        &["go"],
    )
}

/// A history that crosses the compaction threshold: a completed turn produces a
/// `Compacted` event and a `CompactSummary` checkpoint head (spec item 16).
fn scenario_compaction(home: &Path, cwd: &Path) -> Observable {
    // 184_000 tokens is the trigger; the estimator is chars/4, so a ~800k-char
    // prompt clears it with room to keep a small tail.
    let big_prompt = "x".repeat(800_000);
    let provider = CompactProvider::new(vec![vec![text("ok"), done_stop()]], "SUMMARY BODY")
        .with_per_call(usage(3, 4));
    capture(
        home,
        cwd,
        Box::new(provider),
        vec![],
        None,
        true,
        &[big_prompt.as_str()],
    )
}

/// The compaction failure path: the summary call errors, so the full history
/// and this turn's result survive and no `Compacted` event is emitted
/// (spec item 16, failure arm).
fn scenario_compaction_failure(home: &Path, cwd: &Path) -> Observable {
    let big_prompt = "y".repeat(800_000);
    let provider =
        CompactProvider::new(vec![vec![text("ok"), done_stop()]], "UNUSED").fails_summary();
    capture(
        home,
        cwd,
        Box::new(provider),
        vec![],
        None,
        true,
        &[big_prompt.as_str()],
    )
}

/// The skill-read interception: a model `read` of an installed skill's
/// `SKILL.md` is served as the activation block, verbatim (spec item 18).
fn scenario_skill_intercept(home: &Path, cwd: &Path) -> Observable {
    let skills = install_probe_skill(home);
    let provider = FakeProvider::new(vec![
        vec![
            tc_start(0, "call_1", "read"),
            tc_args(
                0,
                &format!("{{\"path\": \"{}\"}}", skills[0].file.display()),
            ),
            done_tools(),
        ],
        vec![text("loaded"), done_stop()],
    ]);
    capture(
        home,
        cwd,
        Box::new(provider),
        vec![read_tool()],
        Some(skills),
        true,
        &["use the probe skill"],
    )
}

/// Install a deterministic probe skill and return the loaded list (with the
/// real on-disk `file` path the read-interception matches on).
fn install_probe_skill(home: &Path) -> Vec<slimcode_app::skills::Skill> {
    let store = SkillStore::new(home, home);
    let skills_dir = home.join("skills").join("probe");
    std::fs::create_dir_all(&skills_dir).expect("skill dir");
    std::fs::write(
        skills_dir.join("SKILL.md"),
        "---\nname: probe\ndescription: a probe skill\n---\nPROBE BODY\n",
    )
    .expect("write skill");
    let _ = store; // installed by hand above; discovery reads it back.
    SkillStore::new(home, home).list().expect("list skills")
}

/// A `read` tool that would return raw file content — the interception must
/// replace it with the skill block, so if it ever runs the baseline drifts.
fn read_tool() -> slimcode_app::Tool {
    slimcode_app::Tool::new(
        "read",
        "read a file",
        serde_json::json!({"type": "object"}),
        |args: serde_json::Value| {
            let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            std::fs::read_to_string(path).map_err(|e| e.to_string())
        },
    )
}

// --- goldens ---------------------------------------------------------------

// The golden is a snapshot file (`tests/baseline_goldens.snap`) holding the
// exact normalized footprint of every scenario on the pre-migration
// `AgentSession`. Later engine tickets re-run this test and must keep it byte
// equal; a drift is a regression, not a refactor. Regenerate deliberately with
// `SLIMCODE_UPDATE_BASELINE=1 cargo test -p slimcode-sdk --test
// equivalence_baseline` and review the diff.

use std::io::Write as _;

/// Render all scenarios into one deterministic snapshot string.
fn render_all() -> String {
    let (_g, home, cwd) = test_env("golden");
    let scenarios: Vec<(&str, Observable)> = vec![
        ("tool_turn", scenario_tool_turn(&home, &cwd)),
        ("error", scenario_error(&home, &cwd)),
        ("cancel", scenario_cancel(&home, &cwd)),
        ("compaction", scenario_compaction(&home, &cwd)),
        (
            "compaction_failure",
            scenario_compaction_failure(&home, &cwd),
        ),
        ("skill_intercept", scenario_skill_intercept(&home, &cwd)),
    ];
    let mut out = String::new();
    for (name, obs) in scenarios {
        out.push_str(&format!("### {name}\n{obs:#?}\n\n"));
    }
    out
}

#[test]
fn equivalence_baseline_matches_golden_snapshot() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/baseline_goldens.snap");
    let rendered = render_all();
    let update = std::env::var("SLIMCODE_UPDATE_BASELINE").is_ok();
    if update || !std::path::Path::new(path).exists() {
        let mut f = std::fs::File::create(path).expect("write baseline snapshot");
        f.write_all(rendered.as_bytes())
            .expect("write baseline bytes");
        eprintln!(
            "wrote baseline snapshot to {path} ({} bytes)",
            rendered.len()
        );
        return;
    }
    let golden = std::fs::read_to_string(path).expect("read baseline snapshot");
    assert_eq!(
        rendered, golden,
        "engine migration drifted from the captured pre-migration baseline"
    );
}
