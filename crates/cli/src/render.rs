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
//! `map_event` in `slimcode-common`.

use std::io::Write;

use slimcode_agent::agent::StopReason;
use slimcode_common::render::{DisplayItem, Renderer, usage_summary};

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
    use slimcode_agent::agent::{AgentEvent, Delta, FinishReason};
    use slimcode_ai::TokenUsage;
    use slimcode_common::render::map_event;

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
}
