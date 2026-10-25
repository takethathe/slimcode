//! Loop guardrails + rollback tests (`.scratch/engine` ticket 08, spec items 8
//! & 19): drive a turn through the public [`Engine`] seam and pin that
//!
//! - `RunConfig::max_iterations` / `timeout` stop a run **under control** at the
//!   boundary with a distinguishable [`StopReason`], while leaving the loop
//!   uncapped when unset;
//! - the bot preset's default `after_turn` [`bot_phase_hooks`] rolls an
//!   interrupted turn (cancelled, guardrail-stopped, or aborted) back to its
//!   [`RunState::initial_message_count`] cursor,
//!   whereas the code path (no such hook) keeps the partial history — the
//!   equivalence gate (`equivalence_baseline`) pins the latter; here we assert
//!   the rollback actually fires.

use std::time::Duration;

use slimcode_app::context::Context;
use slimcode_app::preset::bot_phase_hooks;
use slimcode_app::render::{DisplayItem, Renderer};
use slimcode_app::{
    AgentMessage, CancelToken, Delta, Engine, FinishReason, Message, Preset, Provider,
    ProviderConfig, Role, StopReason, TokenUsage, Tool, ToolSpec,
};

/// A provider that keeps calling the `probe` tool (so only a guardrail, cancel,
/// or its own final answer can end a run). `cancel_on_call` trips the shared
/// token at a 1-based call, `sleep` pads every call for the timeout test.
struct LoopingProvider {
    calls: usize,
    cancel_on_call: Option<usize>,
    cancel: Option<CancelToken>,
    sleep: Option<Duration>,
    stop_after: Option<usize>,
}

impl LoopingProvider {
    fn new() -> Self {
        Self {
            calls: 0,
            cancel_on_call: None,
            cancel: None,
            sleep: None,
            stop_after: None,
        }
    }
    fn cancels_on(mut self, call: usize, token: &CancelToken) -> Self {
        self.cancel_on_call = Some(call);
        self.cancel = Some(token.clone());
        self
    }
    fn sleeps(mut self, d: Duration) -> Self {
        self.sleep = Some(d);
        self
    }
    /// Emit a final `Stop` (no tool call) instead of a tool call once `calls`
    /// reaches `n` — so an uncapped run terminates naturally.
    fn stops_after(mut self, n: usize) -> Self {
        self.stop_after = Some(n);
        self
    }
}

impl Provider for LoopingProvider {
    fn chat(
        &mut self,
        _messages: &[Message],
        _tools: &[ToolSpec],
        _config: &ProviderConfig,
        _cancel: &CancelToken,
        on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
    ) -> Result<Option<TokenUsage>, String> {
        if let Some(d) = self.sleep {
            std::thread::sleep(d);
        }
        self.calls += 1;
        if let (Some(tok), Some(n)) = (&self.cancel, self.cancel_on_call)
            && self.calls == n
        {
            tok.cancel();
        }
        if self.stop_after == Some(self.calls) {
            on_delta(Delta::Text("done".to_string()))?;
            on_delta(Delta::Done(FinishReason::Stop))?;
            return Ok(None);
        }
        on_delta(Delta::ToolCallStart {
            index: 0,
            id: format!("call_{}", self.calls),
            name: "probe".to_string(),
        })?;
        on_delta(Delta::ToolCallArgs {
            index: 0,
            fragment: "{}".to_string(),
        })?;
        on_delta(Delta::Done(FinishReason::ToolCalls))?;
        Ok(None)
    }
}

struct Null;

impl Renderer for Null {
    fn render(&mut self, _item: &DisplayItem) -> Result<(), String> {
        Ok(())
    }
}

fn probe_tool() -> Tool {
    Tool::new("probe", "probe", serde_json::json!({}), |_| {
        Ok("probed".to_string())
    })
}

fn config() -> ProviderConfig {
    ProviderConfig::new("k", "https://example.invalid/v1", "m")
}

fn context(prompt: &str) -> Context {
    Context {
        system: Message::text(Role::System, "sys"),
        messages: vec![AgentMessage::text(Role::User, prompt)],
    }
}

/// `max_iterations` caps the run at exactly that many provider calls and reports
/// a distinguishable stop reason.
#[test]
fn max_iterations_stops_under_control_through_the_engine() {
    let mut provider: Box<dyn Provider + Send> = Box::new(LoopingProvider::new());
    let tools = [probe_tool()];
    let config = config();
    let run_config = slimcode_app::RunConfig {
        parallel_tools: true,
        max_iterations: Some(2),
        timeout: None,
    };
    let mut engine = Engine::new(&mut provider, &tools, &config).with_run_config(run_config);
    let (messages, stop) = engine
        .run_turn(
            context("go"),
            &CancelToken::new(),
            &mut Null,
            &mut |_, _| Ok(()),
        )
        .unwrap();
    assert_eq!(stop, StopReason::MaxIterations);
    // 2 iterations: assistant tool-call + tool result each, plus the initial user.
    assert_eq!(messages.len(), 5);
}

/// With no cap set the same never-stopping provider reaches its own final
/// answer — the loop stays uncapped (the code default).
#[test]
fn unset_guardrails_leave_the_loop_uncapped() {
    let mut provider: Box<dyn Provider + Send> = Box::new(LoopingProvider::new().stops_after(3));
    let tools = [probe_tool()];
    let config = config();
    // No `with_run_config`: the default RunConfig has neither guardrail.
    let mut engine = Engine::new(&mut provider, &tools, &config);
    let (_, stop) = engine
        .run_turn(
            context("go"),
            &CancelToken::new(),
            &mut Null,
            &mut |_, _| Ok(()),
        )
        .unwrap();
    assert_eq!(stop, StopReason::Completed);
}

/// A wall-clock `timeout` stops the run under control at the next boundary with
/// its own stop reason.
#[test]
fn timeout_stops_under_control_through_the_engine() {
    let mut provider: Box<dyn Provider + Send> =
        Box::new(LoopingProvider::new().sleeps(Duration::from_millis(30)));
    let tools = [probe_tool()];
    let config = config();
    let run_config = slimcode_app::RunConfig {
        parallel_tools: true,
        max_iterations: None,
        timeout: Some(Duration::from_millis(10)),
    };
    let mut engine = Engine::new(&mut provider, &tools, &config).with_run_config(run_config);
    let (_, stop) = engine
        .run_turn(
            context("go"),
            &CancelToken::new(),
            &mut Null,
            &mut |_, _| Ok(()),
        )
        .unwrap();
    assert_eq!(stop, StopReason::Timeout);
}

/// The bot preset's rollback hook truncates an interrupted (cancelled) turn back
/// to its starting cursor, so the half-applied history does not linger.
#[test]
fn bot_preset_rolls_back_a_cancelled_turn() {
    let cancel = CancelToken::new();
    // Cancels on the second call, after the first tool round already landed in
    // history (user + assistant + tool = 3 messages).
    let mut provider: Box<dyn Provider + Send> =
        Box::new(LoopingProvider::new().cancels_on(2, &cancel));
    let tools = [probe_tool()];
    let config = config();
    let mut engine =
        Engine::new(&mut provider, &tools, &config).with_preset(Preset::Custom(bot_phase_hooks()));
    let (messages, stop) = engine
        .run_turn(context("go"), &cancel, &mut Null, &mut |_, _| Ok(()))
        .unwrap();
    assert_eq!(stop, StopReason::Cancelled);
    // Rolled back to the initial_message_count cursor: just the user message.
    assert_eq!(
        messages.len(),
        1,
        "the cancelled turn must roll back to the run's starting history"
    );
    assert_eq!(
        messages[0].role(),
        &Role::User,
        "the surviving message is the original user turn"
    );
}

/// The same cancelled turn, run with the code preset's empty bundle (no rollback
/// hook), keeps its partial history — proving the rollback is the hook's doing,
/// not a kernel behavior.
#[test]
fn code_path_keeps_a_cancelled_turns_partial_history() {
    let cancel = CancelToken::new();
    let mut provider: Box<dyn Provider + Send> =
        Box::new(LoopingProvider::new().cancels_on(2, &cancel));
    let tools = [probe_tool()];
    let config = config();
    // `Engine::new` defaults to an empty (no-hook) preset.
    let mut engine = Engine::new(&mut provider, &tools, &config);
    let (messages, stop) = engine
        .run_turn(context("go"), &cancel, &mut Null, &mut |_, _| Ok(()))
        .unwrap();
    assert_eq!(stop, StopReason::Cancelled);
    assert!(
        messages.len() > 1,
        "without the rollback hook the partial turn is adopted ({} messages)",
        messages.len()
    );
}

/// A `max_iterations` stop is an interruption too: the bot preset rolls the
/// capped turn back to its starting cursor, leaving no half-applied history
/// (ticket 08 pairs the guardrails with the rollback hook).
#[test]
fn bot_preset_rolls_back_a_max_iterations_stop() {
    let mut provider: Box<dyn Provider + Send> = Box::new(LoopingProvider::new());
    let tools = [probe_tool()];
    let config = config();
    let run_config = slimcode_app::RunConfig {
        parallel_tools: true,
        max_iterations: Some(2),
        timeout: None,
    };
    let mut engine = Engine::new(&mut provider, &tools, &config)
        .with_run_config(run_config)
        .with_preset(Preset::Custom(bot_phase_hooks()));
    let (messages, stop) = engine
        .run_turn(
            context("go"),
            &CancelToken::new(),
            &mut Null,
            &mut |_, _| Ok(()),
        )
        .unwrap();
    assert_eq!(stop, StopReason::MaxIterations);
    assert_eq!(
        messages.len(),
        1,
        "a guardrail-stopped turn must roll back to the run's starting history"
    );
    assert_eq!(
        messages[0].role(),
        &Role::User,
        "the surviving message is the original user turn"
    );
}

/// The wall-clock `timeout` stop rolls back the same way as the other
/// interruptions.
#[test]
fn bot_preset_rolls_back_a_timeout_stop() {
    let mut provider: Box<dyn Provider + Send> =
        Box::new(LoopingProvider::new().sleeps(Duration::from_millis(30)));
    let tools = [probe_tool()];
    let config = config();
    let run_config = slimcode_app::RunConfig {
        parallel_tools: true,
        max_iterations: None,
        timeout: Some(Duration::from_millis(10)),
    };
    let mut engine = Engine::new(&mut provider, &tools, &config)
        .with_run_config(run_config)
        .with_preset(Preset::Custom(bot_phase_hooks()));
    let (messages, stop) = engine
        .run_turn(
            context("go"),
            &CancelToken::new(),
            &mut Null,
            &mut |_, _| Ok(()),
        )
        .unwrap();
    assert_eq!(stop, StopReason::Timeout);
    assert_eq!(
        messages.len(),
        1,
        "a timeout-stopped turn must roll back to the run's starting history"
    );
    assert_eq!(
        messages[0].role(),
        &Role::User,
        "the surviving message is the original user turn"
    );
}
