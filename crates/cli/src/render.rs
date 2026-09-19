//! CLI `TextRenderer`: the one-shot CLI's renderer over the shared
//! [`Renderer`] trait (ADR-0004). It consumes [`DisplayItem`]s and writes
//! streamed fragments and structural lines to a `Write`.
//!
//! Three kinds of output: live streamed assistant text (printed as it streams,
//! no trailing newline), live streamed reasoning (printed as it streams with a
//! `> ` prefix on every physical line, no trailing newline), and structural
//! lines (tool starts/results, stop markers, turn markers, token usage).
//! Structural lines always start on their own row, even when the preceding
//! streamed line didn't end with a newline. Raw `ToolCallStart`/
//! `ToolCallArgs`/`Done` deltas never reach a renderer — they are suppressed by
//! `map_event` in `slimcode-app`.
//!
//! The same module owns the TUI's adapter: [`TuiAdapter`] implements the same
//! `Renderer` trait but converts each `DisplayItem` into the
//! [`RenderItem`](slimcode_tui::render::RenderItem) the TUI applies
//! (ADR-0014 D1/D2). That is why the TUI crate needs no application-layer
//! dependency of its own.

use std::io::Write;

use std::collections::HashMap;
use std::path::PathBuf;

use slimcode_ai::TokenUsage;
use slimcode_app::render::{DisplayItem, Renderer, usage_summary};
use slimcode_app::skills::{Skill, parse_skill_block, skill_for_read_args};
use slimcode_core::agent::StopReason;
use slimcode_core::session::{AgentMessage, Role};
use slimcode_tui::footer::FooterUsage;
use slimcode_tui::render::RenderItem;

/// Convert one display item into the TUI's vocabulary (ADR-0014 D1).
///
/// Two items are dropped: turn markers (the pi-aligned transcript has none)
/// and stop markers (the TUI renders nothing for a stop). Token usage becomes
/// the TUI's own [`FooterUsage`], so the provider's usage type never reaches
/// the TUI crate.
pub fn to_render_item(item: &DisplayItem) -> Option<RenderItem> {
    Some(match item {
        DisplayItem::Text(t) => RenderItem::Text(t.clone()),
        DisplayItem::Reasoning(t) => RenderItem::Reasoning(t.clone()),
        DisplayItem::ToolStart {
            tool_call_id,
            name,
            arguments,
        } => RenderItem::ToolStart {
            tool_call_id: tool_call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        },
        DisplayItem::ToolResult {
            tool_call_id,
            name,
            ok,
            result,
        } => RenderItem::ToolResult {
            tool_call_id: tool_call_id.clone(),
            name: name.clone(),
            ok: *ok,
            result: result.clone(),
        },
        DisplayItem::Usage(u) => RenderItem::Usage(to_footer_usage(u)),
        DisplayItem::Turn { .. } | DisplayItem::Stop(_) => return None,
    })
}

/// Replay a loaded session's history as the render items the TUI appends to
/// the transcript (the `SessionChanged` clear already happened, so these items
/// build the fresh view back up). The mapping mirrors the streaming path:
/// user messages become prompt boxes, assistant text (non-empty parts) becomes
/// streamed text, each assistant tool call becomes a pending tool block, and
/// every tool result fills the matching block (or keeps its own when the start
/// was never seen). `ok` is not persisted in the log, so it is recovered the
/// way the runner writes errors: a result whose text starts with `Error: ` is a
/// failure (ADR-0012 D1). `System` messages never reach a history (ADR-0012 D3)
/// and are skipped defensively.
pub fn history_to_render_items(messages: &[AgentMessage]) -> Vec<RenderItem> {
    // Assistant tool calls carry the name; the matching tool result only has
    // the call id, so the name is remembered until the result arrives.
    let mut tool_names: HashMap<&str, &str> = HashMap::new();
    // Skill reads were turned into skill active blocks: the tool result text
    // carries the `<skill>` block, so on replay it renders as a `Skill` block
    // (name + inner content, the same split pi's `ParsedSkillBlock` uses) and
    // the paired `read` call's start is suppressed.
    let mut skill_results: HashMap<String, (String, String)> = HashMap::new();
    for message in messages {
        if *message.role() == Role::Tool {
            let text = message.text_content();
            if let Some((name, content)) = parse_skill_block(&text)
                && let Some(id) = message.tool_call_id()
            {
                skill_results.insert(id.to_string(), (name.to_string(), content.to_string()));
            }
        }
    }
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        // A compaction checkpoint replays as a boundary notice, not a prompt
        // box: its summary is what the model reads next, not a user message
        // the user ever typed.
        if let AgentMessage::CompactSummary { tokens_before, .. } = message {
            out.push(RenderItem::Notice(format!(
                "context compacted ({tokens_before} tokens before)"
            )));
            continue;
        }
        match message.role() {
            Role::User => {
                let text = message.text_content();
                // A skill-trigger user message starts with the `<skill>` block
                // (the trigger injects it), so it replays as a skill block
                // rather than a boxed prompt.
                match parse_skill_block(&text) {
                    Some((name, content)) => out.push(RenderItem::Skill {
                        name: name.to_string(),
                        content: content.to_string(),
                    }),
                    None => out.push(RenderItem::UserPrompt(text)),
                }
            }
            Role::Assistant => {
                let text = message.text_content();
                if !text.is_empty() {
                    out.push(RenderItem::Text(text));
                }
                for call in message.tool_calls() {
                    if skill_results.contains_key(call.id.as_str()) {
                        // The read that loaded a skill replays as its active
                        // block; its start is suppressed.
                        continue;
                    }
                    tool_names.insert(call.id.as_str(), call.name.as_str());
                    out.push(RenderItem::ToolStart {
                        tool_call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    });
                }
            }
            Role::Tool => {
                let tool_call_id = message
                    .tool_call_id()
                    .map(str::to_string)
                    .unwrap_or_default();
                if let Some((skill_name, skill_content)) =
                    skill_results.remove(tool_call_id.as_str())
                {
                    out.push(RenderItem::Skill {
                        name: skill_name,
                        content: skill_content,
                    });
                    continue;
                }
                let name = tool_names
                    .remove(tool_call_id.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let result = message.text_content();
                out.push(RenderItem::ToolResult {
                    tool_call_id,
                    name,
                    ok: !result.starts_with("Error: "),
                    result,
                });
            }
            Role::System => {}
        }
    }
    out
}

/// The TUI's view of a provider's cumulative usage (ADR-0014 D1): the CLI does
/// the conversion, so `slimcode-tui` never names the AI type.
pub fn to_footer_usage(usage: &TokenUsage) -> FooterUsage {
    FooterUsage::new(
        usage.prompt_tokens,
        usage.completion_tokens,
        usage.cached_tokens(),
        usage.cache_creation_tokens(),
    )
}

/// The CLI's TUI adapter (ADR-0014 D2): a [`Renderer`] that runs on the turn's
/// worker thread and feeds each item into the library's emit callback. The
/// library owns the channel and the frame loop; the CLI only decides how a
/// `DisplayItem` looks in the TUI.
///
/// With a skills list (and a cwd to resolve read paths against), a `read` of
/// a skill's `SKILL.md` renders as a [`RenderItem::Skill`] active block
/// instead of a `read` tool block: the start is suppressed and the result
/// becomes the skill block (name + inner content, split from the `<skill>`
/// block the runner wrote in place of the file content). Without skills the
/// adapter is a plain mapper.
pub struct TuiAdapter<'a> {
    emit: &'a mut dyn FnMut(RenderItem),
    skills: Vec<Skill>,
    cwd: PathBuf,
    /// `tool_call_id` → skill name for suppressed skill-read starts, paired to
    /// their result when it arrives (parallel batches keep several pending).
    pending_skill: HashMap<String, String>,
}

impl<'a> TuiAdapter<'a> {
    /// Only the tests build a plain adapter; production always uses
    /// [`TuiAdapter::with_skills`], which carries the CLI's skill context.
    #[cfg(test)]
    pub fn new(emit: &'a mut dyn FnMut(RenderItem)) -> Self {
        Self {
            emit,
            skills: Vec::new(),
            cwd: PathBuf::new(),
            pending_skill: HashMap::new(),
        }
    }

    /// A skill-aware adapter: `read` calls naming an installed skill's file
    /// become skill active blocks.
    pub fn with_skills(
        emit: &'a mut dyn FnMut(RenderItem),
        skills: Vec<Skill>,
        cwd: PathBuf,
    ) -> Self {
        Self {
            emit,
            skills,
            cwd,
            pending_skill: HashMap::new(),
        }
    }
}

impl Renderer for TuiAdapter<'_> {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
        match item {
            DisplayItem::ToolStart {
                tool_call_id,
                name,
                arguments,
            } if name == "read" => {
                if let Some(skill) = skill_for_read_args(&self.skills, &self.cwd, arguments) {
                    self.pending_skill
                        .insert(tool_call_id.clone(), skill.name.clone());
                    return Ok(());
                }
            }
            DisplayItem::ToolResult {
                tool_call_id,
                result,
                ..
            } => {
                if let Some(skill_name) = self.pending_skill.remove(tool_call_id) {
                    let content = parse_skill_block(result)
                        .map(|(_, inner)| inner.to_string())
                        .unwrap_or_default();
                    (self.emit)(RenderItem::Skill {
                        name: skill_name,
                        content,
                    });
                    return Ok(());
                }
            }
            _ => {}
        }
        if let Some(render_item) = to_render_item(item) {
            (self.emit)(render_item);
        }
        Ok(())
    }
}

/// The kind of streamed line currently open (no trailing newline yet), if any.
/// Text and reasoning never share a row: when one is open and the other kind
/// arrives, the open row is closed first so the new kind starts on its own row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenLine {
    /// Assistant text, written without a prefix.
    Text,
    /// Reasoning, written with a `> ` prefix on each physical line.
    Reasoning,
}

/// The CLI's renderer: turns [`DisplayItem`]s into one-shot terminal output.
/// Wraps a `&mut dyn Write` so the one-shot path shares it with the shared turn
/// runner.
///
/// With a skills list (and a cwd to resolve read paths against), a `read` of
/// a skill's `SKILL.md` renders as a `[skill] <name>` line instead of a `read`
/// tool block: the start is suppressed and the result prints only the skill
/// name (pi's collapsed skill block; the one-shot has no expand key). Without
/// skills the renderer prints tool blocks verbatim.
pub struct TextRenderer<'a> {
    out: &'a mut dyn Write,
    /// The kind of streamed line currently open (no trailing newline), if any
    /// — the next structural line must start on its own row.
    open: Option<OpenLine>,
    skills: Vec<Skill>,
    cwd: PathBuf,
    /// `tool_call_id` → skill name for suppressed skill-read starts, paired to
    /// their result when it arrives.
    pending_skill: HashMap<String, String>,
}

impl<'a> TextRenderer<'a> {
    /// Wrap a `Write` target. Only the tests build a skill-less renderer;
    /// production uses [`TextRenderer::with_skills`].
    #[cfg(test)]
    pub fn new(out: &'a mut dyn Write) -> Self {
        Self {
            out,
            open: None,
            skills: Vec::new(),
            cwd: PathBuf::new(),
            pending_skill: HashMap::new(),
        }
    }

    /// A skill-aware renderer: `read` calls naming an installed skill's file
    /// become `[skill]` active blocks.
    pub fn with_skills(out: &'a mut dyn Write, skills: Vec<Skill>, cwd: PathBuf) -> Self {
        Self {
            out,
            open: None,
            skills,
            cwd,
            pending_skill: HashMap::new(),
        }
    }
}

impl Renderer for TextRenderer<'_> {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
        // A `read` of a skill's SKILL.md renders as a skill active block
        // instead of a `read` tool block.
        if self.render_skill(item)? {
            return Ok(());
        }
        match item {
            DisplayItem::Text(t) => {
                // Text and reasoning never share a row.
                if self.open == Some(OpenLine::Reasoning) {
                    writeln!(self.out).map_err(|e| e.to_string())?;
                }
                write!(self.out, "{t}").map_err(|e| e.to_string())?;
                self.open = if t.ends_with('\n') {
                    None
                } else {
                    Some(OpenLine::Text)
                };
            }
            DisplayItem::Reasoning(t) => {
                if self.open == Some(OpenLine::Text) {
                    writeln!(self.out).map_err(|e| e.to_string())?;
                }
                // Stream the fragment inline; each physical line carries the
                // `> ` prefix. A fragment continues the open reasoning row
                // (no extra prefix) or, when starting a fresh row, is prefixed.
                let mut at_line_start = self.open != Some(OpenLine::Reasoning);
                for piece in t.split_inclusive('\n') {
                    if at_line_start {
                        write!(self.out, "> ").map_err(|e| e.to_string())?;
                    }
                    write!(self.out, "{piece}").map_err(|e| e.to_string())?;
                    at_line_start = true;
                }
                self.open = if t.ends_with('\n') {
                    None
                } else {
                    Some(OpenLine::Reasoning)
                };
            }
            DisplayItem::Usage(u) => {
                // The pre-migration one-shot printed "\n{tokens}\n" after the
                // event stream; reproduce it verbatim (the leading newline
                // ends an unterminated streamed line and adds a blank row when
                // the last line was terminated).
                writeln!(self.out, "\n{}", usage_summary(u)).map_err(|e| e.to_string())?;
                self.open = None;
            }
            structural => {
                if self.open.is_some() {
                    // The last streamed line didn't end with a newline; start
                    // this structural line on its own row.
                    writeln!(self.out).map_err(|e| e.to_string())?;
                    self.open = None;
                }
                writeln!(self.out, "{}", render_structural(structural))
                    .map_err(|e| e.to_string())?;
            }
        }
        self.out.flush().map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl TextRenderer<'_> {
    /// Render a skill-read pair as a `[skill]` line, returning `true` when
    /// `item` was consumed. A `read` start naming an installed skill's file is
    /// remembered (nothing printed); its result prints `[skill] <name>` only —
    /// the collapsed pi form, since the one-shot has no expand key.
    fn render_skill(&mut self, item: &DisplayItem) -> Result<bool, String> {
        match item {
            DisplayItem::ToolStart {
                tool_call_id,
                name,
                arguments,
            } if name == "read" => {
                if let Some(skill) = skill_for_read_args(&self.skills, &self.cwd, arguments) {
                    self.pending_skill
                        .insert(tool_call_id.clone(), skill.name.clone());
                    return Ok(true);
                }
            }
            DisplayItem::ToolResult { tool_call_id, .. } => {
                if let Some(skill_name) = self.pending_skill.remove(tool_call_id) {
                    if self.open.is_some() {
                        writeln!(self.out).map_err(|e| e.to_string())?;
                        self.open = None;
                    }
                    writeln!(self.out, "  [skill] {skill_name}").map_err(|e| e.to_string())?;
                    self.out.flush().map_err(|e| e.to_string())?;
                    return Ok(true);
                }
            }
            _ => {}
        }
        Ok(false)
    }
}

/// The display text for a structural line (turn marker, reasoning, tool
/// start/result, stop marker). Text and Usage are handled directly in
/// [`TextRenderer::render`] and never routed here.
fn render_structural(item: &DisplayItem) -> String {
    match item {
        DisplayItem::Turn { turn } => format!("── turn {turn} ──"),
        DisplayItem::ToolStart {
            name, arguments, ..
        } => format!("  ▶ {name} {arguments}"),
        DisplayItem::ToolResult {
            name, ok, result, ..
        } => {
            let icon = if *ok { "✔" } else { "✖" };
            format!("  {icon} {name}: {result}")
        }
        DisplayItem::Stop(StopReason::Completed) => "✓ done".to_string(),
        // The one-shot CLI never sets the cancel token, so a Cancelled stop
        // cannot reach it (Esc cancellation is a TUI-only path).
        DisplayItem::Stop(StopReason::Cancelled) => {
            unreachable!("the one-shot CLI never cancels a turn")
        }
        DisplayItem::Text(_) | DisplayItem::Reasoning(_) | DisplayItem::Usage(_) => {
            unreachable!("Text, Reasoning and Usage are handled by TextRenderer::render")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_ai::TokenUsage;
    use slimcode_app::render::map_event;
    use slimcode_app::skills::SkillScope;
    use slimcode_app::testutil::unique_temp_dir;
    use slimcode_core::agent::{AgentEvent, Delta, FinishReason};

    /// Feed a stream of agent events through the shared mapping and the
    /// `TextRenderer`, returning the rendered bytes as a string.
    fn render_stream(events: &[AgentEvent]) -> String {
        let mut buf = Vec::new();
        {
            let mut r = TextRenderer::new(&mut buf);
            for e in events {
                if let Some(item) = map_event(e) {
                    r.render(&item).unwrap();
                }
            }
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn streamed_text_is_written_as_is() {
        assert_eq!(render_stream(&[agent_text("hi")]), "hi");
        assert_eq!(render_stream(&[agent_text("hi ")]), "hi ");
        // Trailing newline is not duplicated by a following stop marker.
        assert_eq!(
            render_stream(&[agent_text("done\n"), agent_stop()]),
            "done\n✓ done\n"
        );
    }

    #[test]
    fn reasoning_renders_as_prefixed_line() {
        // A single reasoning delta leaves the line open (no trailing newline).
        assert_eq!(render_stream(&[agent_reasoning("think")]), "> think");
    }

    #[test]
    fn reasoning_fragments_stream_inline_on_one_line() {
        // Consecutive reasoning deltas append to the same prefixed line rather
        // than each starting a new row.
        assert_eq!(
            render_stream(&[agent_reasoning("The"), agent_reasoning(" user")]),
            "> The user"
        );
    }

    #[test]
    fn reasoning_embedded_newline_reprefixes_each_row() {
        // A newline inside a reasoning delta still starts a prefixed row.
        assert_eq!(
            render_stream(&[agent_reasoning("line one\nline two")]),
            "> line one\n> line two"
        );
    }

    #[test]
    fn reasoning_after_open_text_starts_new_row() {
        // An open assistant-text line must not swallow the reasoning line.
        assert_eq!(
            render_stream(&[agent_text("answer"), agent_reasoning("think")]),
            "answer\n> think"
        );
    }

    #[test]
    fn text_after_open_reasoning_starts_new_row() {
        // An open reasoning line must not swallow the assistant text.
        assert_eq!(
            render_stream(&[agent_reasoning("think"), agent_text("answer")]),
            "> think\nanswer"
        );
    }

    #[test]
    fn raw_tool_deltas_are_suppressed() {
        assert_eq!(
            render_stream(&[
                agent_tc_start(),
                agent_tc_args(),
                agent_done(),
                agent_text("kept"),
            ]),
            "kept"
        );
    }

    #[test]
    fn tool_events_render_name_and_body() {
        assert_eq!(
            render_stream(&[agent_tool_start()]),
            "  ▶ read {\"path\": \"a.txt\"}\n"
        );
        assert_eq!(
            render_stream(&[agent_tool_result(true)]),
            "  ✔ read: hello\n"
        );
        assert_eq!(
            render_stream(&[agent_tool_result(false)]),
            "  ✖ bash: boom\n"
        );
    }

    #[test]
    fn stop_reasons_render() {
        assert_eq!(render_stream(&[agent_stop()]), "✓ done\n");
    }

    #[test]
    fn usage_renders_with_leading_blank_line() {
        let mut buf = Vec::new();
        {
            let mut r = TextRenderer::new(&mut buf);
            let u = TokenUsage {
                prompt_tokens: 5,
                completion_tokens: 3,
                total_tokens: 8,
                ..Default::default()
            };
            r.render(&DisplayItem::Usage(u)).unwrap();
        }
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "\ntokens: 5 prompt (0 cached, 0%) + 3 completion = 8 total\n"
        );
    }

    #[test]
    fn usage_renders_cached_count_when_details_present() {
        let mut buf = Vec::new();
        {
            let mut r = TextRenderer::new(&mut buf);
            let u = TokenUsage {
                prompt_tokens: 20,
                completion_tokens: 4,
                total_tokens: 24,
                prompt_tokens_details: Some(slimcode_ai::wire::PromptTokensDetails {
                    cached_tokens: 16,
                    cache_creation_input_tokens: 4,
                }),
            };
            r.render(&DisplayItem::Usage(u)).unwrap();
        }
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "\ntokens: 20 prompt (16 cached, 80%) + 4 completion = 24 total\n"
        );
    }

    #[test]
    fn structural_line_after_text_starts_new_row() {
        // An unterminated assistant text line must not swallow the structural
        // line that follows it.
        assert_eq!(
            render_stream(&[agent_text("answer"), agent_stop()]),
            "answer\n✓ done\n"
        );
    }

    #[test]
    fn text_trailing_newline_not_duplicated() {
        assert_eq!(
            render_stream(&[agent_text("done\n"), agent_stop()]),
            "done\n✓ done\n"
        );
    }

    #[test]
    fn full_stream_renders_in_expected_order() {
        // Text and structural lines keep the pre-migration byte layout;
        // reasoning now streams inline instead of one line per delta.
        let events = vec![
            AgentEvent::Turn { turn: 1 },
            agent_reasoning("Let me think"),
            agent_text("answer "),
            agent_text("fragment"),
            agent_tc_start(),
            agent_tc_args(),
            agent_tool_start(),
            agent_tool_result(true),
            agent_stop(),
        ];
        assert_eq!(
            render_stream(&events),
            "── turn 1 ──\n> Let me think\nanswer fragment\n  ▶ read {\"path\": \"a.txt\"}\n  ✔ read: hello\n✓ done\n"
        );
    }

    // --- event builders ------------------------------------------------

    fn agent_text(t: &str) -> AgentEvent {
        AgentEvent::Stream(Delta::Text(t.to_string()))
    }
    fn agent_reasoning(t: &str) -> AgentEvent {
        AgentEvent::Stream(Delta::Reasoning(t.to_string()))
    }
    fn agent_tc_start() -> AgentEvent {
        AgentEvent::Stream(Delta::ToolCallStart {
            index: 0,
            id: "c1".into(),
            name: "read".into(),
        })
    }
    fn agent_tc_args() -> AgentEvent {
        AgentEvent::Stream(Delta::ToolCallArgs {
            index: 0,
            fragment: "{}".into(),
        })
    }
    fn agent_done() -> AgentEvent {
        AgentEvent::Stream(Delta::Done(FinishReason::Stop))
    }
    fn agent_tool_start() -> AgentEvent {
        AgentEvent::ToolStart {
            tool_call_id: "call_read".into(),
            name: "read".into(),
            arguments: "{\"path\": \"a.txt\"}".into(),
        }
    }
    fn agent_tool_result(ok: bool) -> AgentEvent {
        AgentEvent::ToolResult {
            tool_call_id: if ok { "call_read" } else { "call_bash" }.into(),
            name: if ok { "read" } else { "bash" }.into(),
            ok,
            result: if ok { "hello" } else { "boom" }.into(),
        }
    }
    fn agent_stop() -> AgentEvent {
        AgentEvent::Stop(StopReason::Completed)
    }

    // --- the TUI adapter (ADR-0014 D1/D2) --------------------------------

    /// Every `DisplayItem` variant maps 1:1 onto the TUI's vocabulary, except
    /// the two the TUI never renders. Kept table-driven so a new display
    /// variant cannot be added without deciding what the TUI shows.
    #[test]
    fn adapter_maps_every_display_item_variant() {
        let usage = TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: Some(slimcode_ai::wire::PromptTokensDetails {
                cached_tokens: 8,
                cache_creation_input_tokens: 2,
            }),
        };
        let cases: Vec<(DisplayItem, Option<RenderItem>)> = vec![
            (
                DisplayItem::Text("hi".to_string()),
                Some(RenderItem::Text("hi".to_string())),
            ),
            (
                DisplayItem::Reasoning("why".to_string()),
                Some(RenderItem::Reasoning("why".to_string())),
            ),
            (
                DisplayItem::ToolStart {
                    tool_call_id: "c1".to_string(),
                    name: "read".to_string(),
                    arguments: "{}".to_string(),
                },
                Some(RenderItem::ToolStart {
                    tool_call_id: "c1".to_string(),
                    name: "read".to_string(),
                    arguments: "{}".to_string(),
                }),
            ),
            (
                DisplayItem::ToolResult {
                    tool_call_id: "c1".to_string(),
                    name: "read".to_string(),
                    ok: true,
                    result: "hello".to_string(),
                },
                Some(RenderItem::ToolResult {
                    tool_call_id: "c1".to_string(),
                    name: "read".to_string(),
                    ok: true,
                    result: "hello".to_string(),
                }),
            ),
            (
                DisplayItem::Usage(usage),
                Some(RenderItem::Usage(FooterUsage::new(10, 5, 8, 2))),
            ),
            (DisplayItem::Turn { turn: 3 }, None),
            (DisplayItem::Stop(StopReason::Completed), None),
            (DisplayItem::Stop(StopReason::Cancelled), None),
        ];
        for (display, expected) in cases {
            assert_eq!(to_render_item(&display), expected, "for {display:?}");
        }
    }

    #[test]
    fn tui_adapter_emits_mapped_items_and_skips_dropped_ones() {
        let mut emitted: Vec<RenderItem> = Vec::new();
        {
            let mut record = |item: RenderItem| emitted.push(item);
            let mut adapter = TuiAdapter::new(&mut record);
            adapter
                .render(&DisplayItem::Text("answer".to_string()))
                .unwrap();
            adapter.render(&DisplayItem::Turn { turn: 1 }).unwrap();
            adapter
                .render(&DisplayItem::Stop(StopReason::Completed))
                .unwrap();
        }
        assert_eq!(emitted, vec![RenderItem::Text("answer".to_string())]);
    }

    #[test]
    fn history_replay_maps_text_messages_in_order() {
        let history = vec![
            AgentMessage::text(Role::User, "hello"),
            AgentMessage::text(Role::Assistant, "hi there"),
            AgentMessage::text(Role::User, "again"),
        ];
        assert_eq!(
            history_to_render_items(&history),
            vec![
                RenderItem::UserPrompt("hello".to_string()),
                RenderItem::Text("hi there".to_string()),
                RenderItem::UserPrompt("again".to_string()),
            ]
        );
    }

    #[test]
    fn history_replay_pairs_tool_calls_with_results() {
        use slimcode_core::session::Message;
        let history = vec![
            AgentMessage::text(Role::User, "check git"),
            AgentMessage::llm(Message {
                role: Role::Assistant,
                parts: vec![slimcode_ai::message::Part::Text {
                    text: String::new(),
                }],
                tool_calls: vec![slimcode_core::session::ToolCall {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    arguments: r#"{"command":"git log"}"#.to_string(),
                }],
                tool_call_id: None,
            }),
            AgentMessage::tool_result("call_1", "commit abc"),
            AgentMessage::text(Role::Assistant, "done"),
        ];
        assert_eq!(
            history_to_render_items(&history),
            vec![
                RenderItem::UserPrompt("check git".to_string()),
                RenderItem::ToolStart {
                    tool_call_id: "call_1".to_string(),
                    name: "bash".to_string(),
                    arguments: r#"{"command":"git log"}"#.to_string(),
                },
                RenderItem::ToolResult {
                    tool_call_id: "call_1".to_string(),
                    name: "bash".to_string(),
                    ok: true,
                    result: "commit abc".to_string(),
                },
                RenderItem::Text("done".to_string()),
            ]
        );
    }

    #[test]
    fn history_replay_renders_skill_trigger_prompt_as_skill_block() {
        let skill_block = "<skill name=\"grill\" location=\"/x/SKILL.md\">\nReferences are relative to /x.\n\nBody.\n</skill>\n\nmy plan";
        let history = vec![
            AgentMessage::text(Role::User, skill_block),
            // A message merely quoting the tag mid-text is an ordinary prompt.
            AgentMessage::text(Role::User, "look at <skill name=\"x\">"),
        ];
        assert_eq!(
            history_to_render_items(&history),
            vec![
                RenderItem::Skill {
                    name: "grill".to_string(),
                    content: "References are relative to /x.\n\nBody.".to_string(),
                },
                RenderItem::UserPrompt("look at <skill name=\"x\">".to_string()),
            ]
        );
    }

    #[test]
    fn history_replay_turns_skill_reads_into_skill_blocks() {
        use slimcode_core::session::Message;
        let skill_block = r#"<skill name="grill" location="/x/SKILL.md">
References are relative to /x.

# Grill a plan

Body.
</skill>"#;
        let history = vec![
            AgentMessage::llm(Message {
                role: Role::Assistant,
                parts: vec![slimcode_ai::message::Part::Text {
                    text: String::new(),
                }],
                tool_calls: vec![slimcode_core::session::ToolCall {
                    id: "call_skill".to_string(),
                    name: "read".to_string(),
                    arguments: r#"{"path": "/x/SKILL.md"}"#.to_string(),
                }],
                tool_call_id: None,
            }),
            AgentMessage::tool_result("call_skill", skill_block),
        ];
        // The read's start is suppressed; its result becomes the skill block
        // (name + inner content, the `location` tag split off).
        assert_eq!(
            history_to_render_items(&history),
            vec![RenderItem::Skill {
                name: "grill".to_string(),
                content: "References are relative to /x.\n\n# Grill a plan\n\nBody.".to_string(),
            }]
        );
    }

    #[test]
    fn history_replay_renders_a_compaction_checkpoint_as_a_notice() {
        let history = vec![
            AgentMessage::text(Role::User, "old"),
            AgentMessage::compact_summary("## Goal\nx", 117_760, None),
            AgentMessage::text(Role::Assistant, "new"),
        ];
        assert_eq!(
            history_to_render_items(&history),
            vec![
                RenderItem::UserPrompt("old".to_string()),
                RenderItem::Notice("context compacted (117760 tokens before)".to_string()),
                RenderItem::Text("new".to_string()),
            ]
        );
    }

    #[test]
    fn tui_adapter_turns_skill_reads_into_skill_blocks() {
        let dir = unique_temp_dir("slimcode-tui-adapter-skill");
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = dir.join("proj");
        let store = slimcode_app::skills::SkillStore::new(&dir.join("home"), &cwd);
        let src = dir.join("grill.md");
        std::fs::write(
            &src,
            "---\nname: grill\ndescription: stress-test a plan\n---\n# Grill\n\nBody.\n",
        )
        .unwrap();
        let skills = vec![store.install(&src, SkillScope::Project).unwrap()];

        let mut emitted: Vec<RenderItem> = Vec::new();
        {
            let mut record = |item: RenderItem| emitted.push(item);
            let mut adapter = TuiAdapter::with_skills(&mut record, skills, cwd.clone());
            // A read of the skill's own file: start suppressed, result becomes
            // the skill block.
            adapter
                .render(&DisplayItem::ToolStart {
                    tool_call_id: "c1".to_string(),
                    name: "read".to_string(),
                    arguments: r#"{"path": ".slimcode/skills/grill/SKILL.md"}"#.to_string(),
                })
                .unwrap();
            adapter
                .render(&DisplayItem::ToolResult {
                    tool_call_id: "c1".to_string(),
                    name: "read".to_string(),
                    ok: true,
                    result: "<skill name=\"grill\" location=\"/x/SKILL.md\">\nReferences are relative to /x.\n\nBody.\n</skill>"
                        .to_string(),
                })
                .unwrap();
            // A plain read still renders as a tool block.
            adapter
                .render(&DisplayItem::ToolStart {
                    tool_call_id: "c2".to_string(),
                    name: "read".to_string(),
                    arguments: r#"{"path": "main.rs"}"#.to_string(),
                })
                .unwrap();
            adapter
                .render(&DisplayItem::ToolResult {
                    tool_call_id: "c2".to_string(),
                    name: "read".to_string(),
                    ok: true,
                    result: "fn main() {}".to_string(),
                })
                .unwrap();
        }
        assert_eq!(
            emitted,
            vec![
                RenderItem::Skill {
                    name: "grill".to_string(),
                    content: "References are relative to /x.\n\nBody.".to_string(),
                },
                RenderItem::ToolStart {
                    tool_call_id: "c2".to_string(),
                    name: "read".to_string(),
                    arguments: r#"{"path": "main.rs"}"#.to_string(),
                },
                RenderItem::ToolResult {
                    tool_call_id: "c2".to_string(),
                    name: "read".to_string(),
                    ok: true,
                    result: "fn main() {}".to_string(),
                },
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_renderer_prints_skill_reads_as_skill_blocks() {
        let dir = unique_temp_dir("slimcode-text-renderer-skill");
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = dir.join("proj");
        let store = slimcode_app::skills::SkillStore::new(&dir.join("home"), &cwd);
        let src = dir.join("grill.md");
        std::fs::write(
            &src,
            "---\nname: grill\ndescription: stress-test a plan\n---\n# Grill\n\nBody.\n",
        )
        .unwrap();
        let skills = vec![store.install(&src, SkillScope::Project).unwrap()];

        let mut buf = Vec::new();
        {
            let mut r = TextRenderer::with_skills(&mut buf, skills, cwd.clone());
            r.render(&DisplayItem::ToolStart {
                tool_call_id: "c1".to_string(),
                name: "read".to_string(),
                arguments: r#"{"path": ".slimcode/skills/grill/SKILL.md"}"#.to_string(),
            })
            .unwrap();
            r.render(&DisplayItem::ToolResult {
                tool_call_id: "c1".to_string(),
                name: "read".to_string(),
                ok: true,
                result: "<skill name=\"grill\" location=\"/x/SKILL.md\">line1\n</skill>"
                    .to_string(),
            })
            .unwrap();
        }
        let out = String::from_utf8(buf).unwrap();
        // pi's collapsed skill block: the name only, no skill content.
        assert_eq!(out, "  [skill] grill\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_replay_marks_error_results_as_failed() {
        use slimcode_core::session::Message;
        let history = vec![
            AgentMessage::llm(Message {
                role: Role::Assistant,
                parts: vec![slimcode_ai::message::Part::Text {
                    text: String::new(),
                }],
                tool_calls: vec![slimcode_core::session::ToolCall {
                    id: "call_2".to_string(),
                    name: "bash".to_string(),
                    arguments: "{}".to_string(),
                }],
                tool_call_id: None,
            }),
            AgentMessage::tool_result("call_2", "Error: boom"),
        ];
        let items = history_to_render_items(&history);
        assert_eq!(
            items[1],
            RenderItem::ToolResult {
                tool_call_id: "call_2".to_string(),
                name: "bash".to_string(),
                ok: false,
                result: "Error: boom".to_string(),
            }
        );
    }

    #[test]
    fn history_replay_skips_system_and_keeps_text_before_tool_calls() {
        use slimcode_core::session::Message;
        let history = vec![
            AgentMessage::text(Role::System, "be helpful"),
            AgentMessage::llm(Message {
                role: Role::Assistant,
                parts: vec![slimcode_ai::message::Part::Text {
                    text: "reasoning aloud".to_string(),
                }],
                tool_calls: vec![slimcode_core::session::ToolCall {
                    id: "call_3".to_string(),
                    name: "read".to_string(),
                    arguments: "{}".to_string(),
                }],
                tool_call_id: None,
            }),
        ];
        let items = history_to_render_items(&history);
        assert_eq!(
            items,
            vec![
                RenderItem::Text("reasoning aloud".to_string()),
                RenderItem::ToolStart {
                    tool_call_id: "call_3".to_string(),
                    name: "read".to_string(),
                    arguments: "{}".to_string(),
                },
            ]
        );
    }

    #[test]
    fn history_replay_orphan_tool_result_becomes_its_own_block() {
        // A repaired tool result (ADR-0012) has no matching start in the
        // history; it must still render as its own block instead of panicking
        // or being dropped.
        let history = vec![AgentMessage::tool_result("call_9", "Error: interrupted")];
        assert_eq!(
            history_to_render_items(&history),
            vec![RenderItem::ToolResult {
                tool_call_id: "call_9".to_string(),
                name: "tool".to_string(),
                ok: false,
                result: "Error: interrupted".to_string(),
            }]
        );
    }
}
