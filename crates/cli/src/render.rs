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

use slimcode_ai::TokenUsage;
use slimcode_app::render::{DisplayItem, Renderer, usage_summary};
use slimcode_core::agent::StopReason;
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
pub struct TuiAdapter<'a> {
    emit: &'a mut dyn FnMut(RenderItem),
}

impl<'a> TuiAdapter<'a> {
    pub fn new(emit: &'a mut dyn FnMut(RenderItem)) -> Self {
        Self { emit }
    }
}

impl Renderer for TuiAdapter<'_> {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
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
pub struct TextRenderer<'a> {
    out: &'a mut dyn Write,
    /// The kind of streamed line currently open (no trailing newline), if any
    /// — the next structural line must start on its own row.
    open: Option<OpenLine>,
}

impl<'a> TextRenderer<'a> {
    /// Wrap a `Write` target.
    pub fn new(out: &'a mut dyn Write) -> Self {
        Self { out, open: None }
    }
}

impl Renderer for TextRenderer<'_> {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
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
}
