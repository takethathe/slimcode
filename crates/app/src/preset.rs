//! Built-in default hooks for the **code-type preset** (ADR-0031 D5, engine
//! ticket 03).
//!
//! The product behaviors slimcode ships with — context assembly, JSONL
//! persistence, title inference, turn-end compaction, the skill-read
//! interception — used to be welded inline into `AgentSession::prompt`. ADR-0031
//! demotes them to *default hooks a host can replace or disable* that enjoy no
//! privileged internals: a host's custom hook and slimcode's default are the
//! same kind of thing.
//!
//! This module holds the pieces that express naturally as **seven-phase
//! [`PhaseHooks`]**. Today that is the skill-read interception, which rides the
//! `pre_tool` phase (ADR-0031 D2: "the skill-read interception moves onto
//! `pre_tool`'s `Skip`"). The remaining code behaviors stay wired at the SDK's
//! turn seam — context assembly is the input the loop runs over, persistence is
//! driven by the message-event sink (the only boundary that keeps the emitted
//! event order stable, and thus the equivalence gate), and compaction / closing
//! run after the loop. [`code_phase_hooks`] is the bundle [`AgentSession`]
//! registers, so swapping in an empty bundle disables the interception without
//! touching the kernel.

use std::path::{Path, PathBuf};

use slimcode_core::agent::{
    AbortReason, AfterTurn, CancelToken, PhaseHooks, PreTool, Provider, ProviderConfig, RunSpec,
    RunState, StopReason, ToolHookAction,
};

use crate::AgentMessage;
use crate::Session;
use crate::compaction;
use crate::context::skill_loaded_in;
use crate::skills::{Skill, skill_for_read_args, skill_prompt};

/// The `pre_tool` phase hook that intercepts a model `read` of a skill's own
/// `SKILL.md` and serves it as the skill activation block instead of the raw
/// file (ticket 05's discovery, now a default phase hook).
///
/// It decides per call (called once per `tool_call` in model order, the whole
/// batch before anything dispatches): a `read` whose `path` argument matches an
/// installed skill's `SKILL.md` returns [`ToolHookAction::Skip`] carrying the
/// activation text (so the tool never runs yet still yields a paired tool
/// result); anything else returns [`ToolHookAction::Execute`].
///
/// `history` is the message list in play **at the turn's start** (the CLI's
/// `history_for_skill_dedup`), used for the already-loaded dedup. It is a
/// frozen snapshot rather than the live [`RunState::messages`] so a second read
/// of the same skill within one turn still reports `already_loaded` exactly as
/// the pre-pipeline `before_tool` did — the equivalence gate.
pub fn skill_read_intercept(
    skills: Vec<Skill>,
    cwd: PathBuf,
    history: Vec<AgentMessage>,
) -> PreTool<'static> {
    PreTool(Box::new(
        move |_spec: &RunSpec, state: &mut RunState, index: usize| {
            // The batch's assistant message is the last history entry at
            // `pre_tool` time (no results pushed yet). Clone its calls so the
            // read does not hold a borrow of `state` while deciding.
            let calls = state
                .messages
                .last()
                .map(|m| m.tool_calls().to_vec())
                .unwrap_or_default();
            let Some(call) = calls.get(index) else {
                return Ok(ToolHookAction::Execute);
            };
            if call.name != "read" {
                return Ok(ToolHookAction::Execute);
            }
            let Some(skill) = skill_for_read_args(&skills, cwd.as_path(), &call.arguments) else {
                return Ok(ToolHookAction::Execute);
            };
            let already_loaded = skill_loaded_in(&history, skill);
            Ok(ToolHookAction::Skip {
                result: Ok(skill_prompt(skill, None, already_loaded)),
            })
        },
    ))
}

/// The code preset's default [`PhaseHooks`] bundle (ADR-0031 D5): today only
/// the `pre_tool` skill-read interception. A host that wants different behavior
/// passes its own bundle (or an empty one, `PhaseHooks::default()`, to disable
/// it) instead. Inert with an empty skill list, exactly as the old inline
/// `before_tool` was.
pub fn code_phase_hooks(
    skills: &[Skill],
    cwd: &Path,
    history: Vec<AgentMessage>,
) -> PhaseHooks<'static> {
    let skills = skills.to_vec();
    let cwd = cwd.to_path_buf();
    PhaseHooks {
        pre_tool: vec![skill_read_intercept(skills, cwd, history)],
        ..PhaseHooks::default()
    }
}

/// The `after_turn` rollback hook a bot-type preset registers (ADR-0031 D5,
/// engine ticket 08): when a turn is interrupted — the caller cancelled
/// ([`StopReason::Cancelled`]), a loop guardrail fired ([`StopReason::MaxIterations`]
/// / [`StopReason::Timeout`]), or a phase aborted (an [`AbortReason`] stashed in
/// [`RunState::extensions`]) — the history is truncated back to the run's
/// [`RunState::initial_message_count`] cursor, so a half-applied turn leaves the
/// session exactly where it started. A clean [`StopReason::Completed`] keeps the
/// full history.
///
/// The kernel never rolls back on its own (see [`RunState::initial_message_count`]),
/// so a preset that does not register this hook (the code preset) is unaffected
/// — the equivalence gate.
pub fn rollback_after_turn() -> AfterTurn<'static> {
    AfterTurn(Box::new(
        |_spec: &RunSpec, state: &mut RunState, stop: &StopReason| {
            let interrupted = matches!(
                stop,
                StopReason::Cancelled | StopReason::MaxIterations | StopReason::Timeout
            ) || state.extensions.get::<AbortReason>().is_some();
            if interrupted {
                state.messages.truncate(state.initial_message_count);
            }
            Ok(())
        },
    ))
}

/// The **bot-type** default [`PhaseHooks`] bundle (ADR-0031 D5, engine ticket
/// 08): the rollback-on-interrupt `after_turn` hook, and nothing else. A host
/// picks it through `Engine::with_preset(Preset::Custom(bot_phase_hooks()))`; the
/// loop guardrails that pair with it live on [`RunConfig`] (`max_iterations` /
/// `timeout`), which a bot run sets while the code run leaves unset.
///
/// An empty bundle (or the code preset's [`code_phase_hooks`]) never truncates
/// history, so the code path is byte-identical to before this hook existed.
pub fn bot_phase_hooks() -> PhaseHooks<'static> {
    PhaseHooks {
        after_turn: vec![rollback_after_turn()],
        ..PhaseHooks::default()
    }
}

/// Everything a turn-end component needs to work over the finished history:
/// the provider operand (a consolidation spends its own request), the session
/// it rewrites in place, the run's [`ProviderConfig`] and [`CancelToken`],
/// whether the call is forced (`compact_now`) or threshold-gated (auto), and a
/// best-effort `persist` the host wires to its own storage (the component must
/// not reach into session internals to write the log).
pub struct TurnEndCtx<'a> {
    pub provider: &'a mut Box<dyn Provider + Send>,
    pub config: &'a ProviderConfig,
    pub cancel: &'a CancelToken,
    pub session: &'a mut Session,
    pub force: bool,
    pub persist: &'a mut dyn FnMut(&Session, &AgentMessage),
}

/// The orchestration-layer step that runs once after a completed turn — auto
/// (threshold-gated) or forced (`compact_now`). It returns
/// `Ok(Some((before, after)))` when it replaced history (the token counts before
/// / after), `Ok(None)` when it ran but changed nothing, or `Err` to surface a
/// failure (auto swallows it, `compact_now` propagates it).
///
/// The code preset's default is [`compaction_turn_end`]; a host swaps in its own
/// consolidation, or disables the step with [`noop_turn_end`]. This is
/// deliberately **not** a core `after_turn` phase hook: compaction needs the
/// provider operand and the host's storage, neither of which a loop-phase hook
/// carries (ADR-0031 D5, engine ticket 04).
pub type TurnEndComponent =
    dyn Fn(&mut TurnEndCtx) -> Result<Option<(usize, usize)>, String> + Send + Sync;

/// The code preset's default [`TurnEndComponent`]: threshold-gated (or forced)
/// history compaction. Byte-for-byte the behavior the SDK carried inline before
/// ADR-0031 (the equivalence gate) — summarize the old span, replace it, persist
/// the checkpoint best-effort, fold the summary request's own cost into
/// `session.usage` (ADR-0018 D4), and report the token change.
pub fn compaction_turn_end(ctx: &mut TurnEndCtx) -> Result<Option<(usize, usize)>, String> {
    if !ctx.force && !compaction::should_compact(&ctx.session.messages) {
        return Ok(None);
    }
    let before = compaction::estimate_total_tokens(&ctx.session.messages);
    // The summary request spends tokens too (ADR-0018 D4): snapshot the
    // provider total so the compaction's own cost lands in `session.usage`.
    let usage_before = ctx.provider.total_usage();
    let compacted =
        compaction::compact_messages(ctx.provider, ctx.config, ctx.cancel, &ctx.session.messages)?;
    ctx.session.messages = compacted;
    // Persist the checkpoint as the boundary it is: the log is append-only and
    // keeps the records it replaced, but the load path drops everything before
    // the newest checkpoint, so a reload feeds the model the same compacted
    // view (ADR-0020 D2). Best-effort, like every other append.
    if let Some(checkpoint) = ctx.session.messages.first() {
        (ctx.persist)(ctx.session, checkpoint);
    }
    ctx.session.usage = ctx
        .session
        .usage
        .saturating_add(&ctx.provider.total_usage().saturating_sub(&usage_before));
    let after = compaction::estimate_total_tokens(&ctx.session.messages);
    Ok(Some((before, after)))
}

/// A [`TurnEndComponent`] that does nothing: wiring it in disables the turn-end
/// step (auto-compaction never fires and `compact_now` reports no change) while
/// leaving the loop and history handling untouched.
pub fn noop_turn_end(ctx: &mut TurnEndCtx) -> Result<Option<(usize, usize)>, String> {
    let _ = ctx;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillScope;
    use serde_json::json;
    use slimcode_core::agent::{
        AgentRunner, CancelToken, Delta, FinishReason, Provider, ProviderConfig, RunConfig,
        TokenUsage, Tool, ToolSpec,
    };
    use slimcode_core::session::{Message, Role};

    fn skill(name: &str, file: PathBuf) -> Skill {
        Skill {
            name: name.to_string(),
            description: "a probe skill".to_string(),
            disable_model_invocation: false,
            body: "PROBE BODY".to_string(),
            scope: SkillScope::User,
            dir: file.parent().unwrap().to_path_buf(),
            file,
        }
    }

    /// Scripted provider: emits one `read` tool call (with `read_arguments` as
    /// its JSON args) on the first request, then a final answer.
    struct Scripted {
        calls: usize,
        read_arguments: String,
    }
    impl Scripted {
        fn new(read_arguments: String) -> Self {
            Self {
                calls: 0,
                read_arguments,
            }
        }
    }
    impl Provider for Scripted {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[ToolSpec],
            _config: &ProviderConfig,
            _cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.calls += 1;
            if self.calls == 1 {
                on_delta(Delta::ToolCallStart {
                    index: 0,
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                })?;
                on_delta(Delta::ToolCallArgs {
                    index: 0,
                    fragment: self.read_arguments.clone(),
                })?;
                on_delta(Delta::Done(FinishReason::ToolCalls))?;
            } else {
                on_delta(Delta::Text("loaded".to_string()))?;
                on_delta(Delta::Done(FinishReason::Stop))?;
            }
            Ok(None)
        }
    }

    /// A `read` tool that echoes `RAW:<path>` — so if the interception fails to
    /// `Skip`, the history shows the tool ran instead of the activation block.
    fn read_tool() -> Tool {
        Tool::new(
            "read",
            "read a file",
            json!({"type": "object"}),
            |args: serde_json::Value| {
                let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
                Ok(format!("RAW:{path}"))
            },
        )
    }

    fn run_once(skills: Vec<Skill>, cwd: PathBuf, read_path: &str) -> Vec<AgentMessage> {
        let history = vec![AgentMessage::text(Role::User, "activate the skill")];
        let phases = code_phase_hooks(&skills, &cwd, history.clone());
        let tools = [read_tool()];
        let config = ProviderConfig::new("k", "https://example.invalid/v1", "m");
        let cancel = CancelToken::new();
        let mut sink = |_e| Ok(());
        let mut runner =
            AgentRunner::new(&tools, RunConfig::default(), &config, &cancel, &mut sink);
        runner.phases = phases;
        let system = Message::text(Role::System, "be helpful");
        let mut provider = Scripted::new(json!({ "path": read_path }).to_string());
        runner.run(&mut provider, &system, history).unwrap().0
    }

    /// Write a real `SKILL.md` under a fresh temp dir and return (dir, file).
    fn temp_skill(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("slimcode-preset-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("SKILL.md");
        std::fs::write(&file, "---\nname: probe\n---\nbody").unwrap();
        (dir, file)
    }

    #[test]
    fn read_of_a_skill_file_is_served_as_the_activation_block() {
        let (dir, file) = temp_skill("skill-hit");
        let path = file.to_string_lossy().to_string();
        let messages = run_once(vec![skill("probe", file.clone())], dir.clone(), &path);
        let tool_result = messages
            .iter()
            .find(|m| m.role() == &Role::Tool)
            .expect("a paired tool result entered history");
        let text = tool_result.text_content();
        assert!(
            text.contains("<skill name=\"probe\""),
            "activation block expected, got: {text}"
        );
        assert!(
            !text.contains("RAW:"),
            "the read tool must not have run: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_that_is_not_a_skill_executes_normally() {
        let (dir, file) = temp_skill("skill-miss");
        // Read an unrelated path: no skill matches, so the tool runs.
        let messages = run_once(
            vec![skill("probe", file.clone())],
            dir.clone(),
            "/other.txt",
        );
        let tool_result = messages
            .iter()
            .find(|m| m.role() == &Role::Tool)
            .expect("a tool result entered history");
        assert!(
            tool_result.text_content().contains("RAW:/other.txt"),
            "the read tool should have run, got: {}",
            tool_result.text_content()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
