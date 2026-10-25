//! App-layer equivalence baseline (`.scratch/engine` ticket 01, spec Testing
//! Decisions item 13): pins the *shared turn loop* seam
//! ([`slimcode_app::runner::run_turn`] / [`run_turn_with_pipeline`]) — the ordered
//! [`DisplayItem`] stream and the returned history — against a snapshot file.
//!
//! The SDK baseline (`crates/sdk/tests/equivalence_baseline.rs`) pins the whole
//! product surface; this one pins the lower seam both frontends share, so the
//! seven-phase rewire in later tickets must keep the loop's external output
//! byte-identical. Regenerate with
//! `SLIMCODE_UPDATE_BASELINE=1 cargo test -p slimcode-app --test equivalence_baseline`.

use std::io::Write as _;

use slimcode_app::render::{DisplayItem, Renderer};
use slimcode_app::runner::{run_turn, run_turn_with_pipeline};
use slimcode_app::{
    AgentMessage, CancelToken, Delta, FinishReason, Message, PhaseHooks, PreTool, Provider,
    ProviderConfig, Role, RunConfig, Tool, ToolHookAction, ToolSpec,
};

/// Scripted provider: pops the next delta batch per call.
struct FakeProvider {
    script: Vec<Vec<Delta>>,
    calls: usize,
}

impl FakeProvider {
    fn new(script: Vec<Vec<Delta>>) -> Self {
        Self { script, calls: 0 }
    }
}

impl Provider for FakeProvider {
    fn chat(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        _cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<slimcode_app::TokenUsage>, String> {
        let batch = self.script.get(self.calls).cloned().unwrap_or_default();
        self.calls += 1;
        for d in batch {
            on_delta(d)?;
        }
        Ok(None)
    }
}

/// Recording renderer: collects every display item as a compact string.
struct Sink {
    items: Vec<String>,
}

impl Renderer for Sink {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
        self.items.push(match item {
            DisplayItem::Turn { turn } => format!("turn({turn})"),
            DisplayItem::Reasoning(t) => format!("reasoning({t})"),
            DisplayItem::Text(t) => format!("text({t})"),
            DisplayItem::ToolStart {
                tool_call_id,
                name,
                arguments,
            } => format!("toolstart({tool_call_id},{name},{arguments})"),
            DisplayItem::ToolResult {
                tool_call_id,
                name,
                ok,
                result,
            } => format!("toolresult({tool_call_id},{name},ok={ok},{result})"),
            DisplayItem::Stop(r) => format!("stop({r:?})"),
            DisplayItem::Usage(u) => format!("usage({})", u.total_tokens),
        });
        Ok(())
    }
}

fn cfg() -> ProviderConfig {
    ProviderConfig::new("k", "https://example.invalid/v1", "m")
}

fn probe_tool() -> Tool {
    Tool::new("probe", "probe", serde_json::json!({}), |_| {
        Ok("probed".to_string())
    })
}

fn t(s: &str) -> Delta {
    Delta::Text(s.to_string())
}
fn tcs(i: usize, id: &str, name: &str) -> Delta {
    Delta::ToolCallStart {
        index: i,
        id: id.into(),
        name: name.into(),
    }
}
fn tca(i: usize, f: &str) -> Delta {
    Delta::ToolCallArgs {
        index: i,
        fragment: f.into(),
    }
}
fn stop() -> Delta {
    Delta::Done(FinishReason::Stop)
}
fn tools() -> Delta {
    Delta::Done(FinishReason::ToolCalls)
}

fn norm_msg(m: &AgentMessage) -> String {
    format!("{}::{}", role(m), m.text_content())
}
fn role(m: &AgentMessage) -> &'static str {
    match m.role() {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// One run's footprint: the ordered display stream and the returned history.
/// The fields are read only through the derived `Debug` that renders the
/// snapshot, which the `dead_code` lint does not credit.
#[allow(dead_code)]
#[derive(Debug)]
struct Foot {
    displays: Vec<String>,
    history: Vec<String>,
    stop: String,
}

fn render_all() -> String {
    // A tool-calling turn then a final answer: pins the loop's exact stream and
    // history in model order.
    let foot_tool = {
        let mut p = FakeProvider::new(vec![
            vec![
                t("checking "),
                tcs(0, "call_1", "probe"),
                tca(0, "{}"),
                tools(),
            ],
            vec![t("done"), stop()],
        ]);
        let mut sink = Sink { items: Vec::new() };
        let system = Message::text(Role::System, "sys");
        let ctx = slimcode_app::context::Context {
            system,
            messages: vec![AgentMessage::text(Role::User, "weather?")],
        };
        let (hist, stopr) = run_turn(
            &mut p,
            &[probe_tool()],
            ctx,
            &RunConfig::default(),
            &cfg(),
            &CancelToken::new(),
            &mut sink,
            &mut |_, _| Ok(()),
        )
        .expect("run_turn");
        Foot {
            displays: sink.items,
            history: hist.iter().map(norm_msg).collect(),
            stop: format!("{stopr:?}"),
        }
    };

    // A `pre_tool` phase Skip: the tool never runs; its supplied result pairs in.
    let foot_skip = {
        let mut p = FakeProvider::new(vec![
            vec![tcs(0, "call_1", "probe"), tca(0, "{}"), tools()],
            vec![t("done"), stop()],
        ]);
        let mut sink = Sink { items: Vec::new() };
        let mut phases = PhaseHooks::default();
        phases.pre_tool.push(PreTool(Box::new(|_spec, _state, _i| {
            Ok(ToolHookAction::Skip {
                result: Ok("SKIPPED".to_string()),
            })
        })));
        let (hist, stopr) = run_turn_with_pipeline(
            &mut p,
            &[probe_tool()],
            slimcode_app::context::Context {
                system: Message::text(Role::System, "sys"),
                messages: vec![AgentMessage::text(Role::User, "go")],
            },
            &RunConfig::default(),
            &cfg(),
            &CancelToken::new(),
            &mut sink,
            &mut |_, _| Ok(()),
            phases,
            None,
        )
        .expect("run_turn_with_pipeline");
        Foot {
            displays: sink.items,
            history: hist.iter().map(norm_msg).collect(),
            stop: format!("{stopr:?}"),
        }
    };

    let mut out = String::new();
    out.push_str(&format!("### tool_turn\n{foot_tool:#?}\n\n"));
    out.push_str(&format!("### skip_hook\n{foot_skip:#?}\n\n"));
    out
}

#[test]
fn app_baseline_matches_golden_snapshot() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/baseline_goldens.snap");
    let rendered = render_all();
    let update = std::env::var("SLIMCODE_UPDATE_BASELINE").is_ok();
    if update || !std::path::Path::new(path).exists() {
        let mut f = std::fs::File::create(path).expect("write app baseline snapshot");
        f.write_all(rendered.as_bytes())
            .expect("write app baseline bytes");
        eprintln!("wrote app baseline snapshot to {path}");
        return;
    }
    let golden = std::fs::read_to_string(path).expect("read app baseline snapshot");
    assert_eq!(
        rendered, golden,
        "shared turn loop drifted from the captured pre-migration baseline"
    );
}
