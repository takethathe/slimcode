//! CLI `TextRenderer`: the one-shot CLI's renderer over the shared
//! [`Renderer`] trait (ADR-0004). It consumes [`DisplayItem`]s and writes
//! text lines / streamed fragments to a `Write`, byte-identical to the
//! pre-migration output (spec user story 36) — only its timing changes, since
//! the shared runner now streams items live.
//!
//! Two kinds of output: live text (printed as it streams, no trailing
//! newline) and structural lines (tool starts/results, stop markers, turn
//! markers, reasoning). Structural lines always start on their own row, even
//! when the preceding assistant text didn't end with a newline. Raw
//! `ToolCallStart`/`ToolCallArgs`/`Done` deltas never reach a renderer — they
//! are suppressed by `map_event` in `slimcode-common`.

use std::io::Write;

use slimcode_agent::agent::StopReason;
use slimcode_ai::TokenUsage;
use slimcode_common::render::{DisplayItem, Renderer};

/// The CLI's renderer: turns [`DisplayItem`]s into the same bytes the old
/// post-hoc renderer produced. Wraps a `&mut dyn Write` so the one-shot path
/// shares it with the shared turn runner.
pub struct TextRenderer<'a> {
    out: &'a mut dyn Write,
    /// Whether the last output was streamed assistant text without a trailing
    /// newline — the next structural line must start on its own row.
    text_line_open: bool,
}

impl<'a> TextRenderer<'a> {
    /// Wrap a `Write` target.
    pub fn new(out: &'a mut dyn Write) -> Self {
        Self {
            out,
            text_line_open: false,
        }
    }
}

impl Renderer for TextRenderer<'_> {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
        match item {
            DisplayItem::Text(t) => {
                write!(self.out, "{t}").map_err(|e| e.to_string())?;
                self.text_line_open = !t.ends_with('\n');
            }
            DisplayItem::Usage(u) => {
                // The pre-migration one-shot printed "\n{tokens}\n" after the
                // event stream; reproduce it verbatim (the leading newline
                // ends an unterminated assistant line and adds a blank row
                // when the last line was terminated).
                writeln!(self.out, "\n{}", render_usage(u)).map_err(|e| e.to_string())?;
                self.text_line_open = false;
            }
            structural => {
                if self.text_line_open {
                    // The last assistant text didn't end with a newline; start
                    // this structural line on its own row.
                    writeln!(self.out).map_err(|e| e.to_string())?;
                    self.text_line_open = false;
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
        DisplayItem::Reasoning(t) => format!("> {t}"),
        DisplayItem::ToolStart { name, arguments } => format!("  ▶ {name} {arguments}"),
        DisplayItem::ToolResult { name, ok, result } => {
            let icon = if *ok { "✔" } else { "✖" };
            format!("  {icon} {name}: {result}")
        }
        DisplayItem::Stop(StopReason::Completed) => "✓ done".to_string(),
        DisplayItem::Stop(StopReason::MaxIterations) => {
            "⚠ stopped: max iterations reached".to_string()
        }
        DisplayItem::Text(_) | DisplayItem::Usage(_) => {
            unreachable!("Text and Usage are handled by TextRenderer::render")
        }
    }
}

/// Render token usage as a summary line (used by the `DisplayItem::Usage`
/// entry after a one-shot run).
pub fn render_usage(u: &TokenUsage) -> String {
    format!(
        "tokens: {} prompt + {} completion = {} total",
        u.prompt_tokens, u.completion_tokens, u.total_tokens
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_agent::agent::{AgentEvent, Delta, FinishReason};
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

    /// The pre-migration renderer (old `render_event` + old `render_events`),
    /// kept inline as the byte-identity reference for the migration.
    fn old_render_stream(events: &[AgentEvent]) -> String {
        let mut out = String::new();
        let mut text_line_open = false;
        for e in events {
            let rt = old_render_event(e);
            if let Some((text, streamed)) = rt {
                if streamed {
                    out.push_str(&text);
                    text_line_open = !text.ends_with('\n');
                } else {
                    if text_line_open {
                        out.push('\n');
                        text_line_open = false;
                    }
                    out.push_str(&text);
                    out.push('\n');
                }
            }
        }
        out
    }

    fn old_render_event(e: &AgentEvent) -> Option<(String, bool)> {
        let (text, streamed) = match e {
            AgentEvent::Turn { turn } => (format!("── turn {turn} ──"), false),
            AgentEvent::Stream(Delta::Reasoning(t)) => (format!("> {t}"), false),
            AgentEvent::Stream(Delta::Text(t)) => (t.clone(), true),
            AgentEvent::Stream(Delta::ToolCallStart { .. })
            | AgentEvent::Stream(Delta::ToolCallArgs { .. })
            | AgentEvent::Stream(Delta::Done(_)) => return None,
            AgentEvent::ToolStart { name, arguments } => (format!("  ▶ {name} {arguments}"), false),
            AgentEvent::ToolResult { name, ok, result } => {
                let icon = if *ok { "✔" } else { "✖" };
                (format!("  {icon} {name}: {result}"), false)
            }
            AgentEvent::Stop(StopReason::Completed) => ("✓ done".to_string(), false),
            AgentEvent::Stop(StopReason::MaxIterations) => {
                ("⚠ stopped: max iterations reached".to_string(), false)
            }
        };
        Some((text, streamed))
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
        assert_eq!(render_stream(&[agent_reasoning("think")]), "> think\n");
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
        assert_eq!(
            render_stream(&[AgentEvent::Stop(StopReason::MaxIterations)]),
            "⚠ stopped: max iterations reached\n"
        );
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
            };
            r.render(&DisplayItem::Usage(u)).unwrap();
        }
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "\ntokens: 5 prompt + 3 completion = 8 total\n"
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
    fn byte_identical_to_old_renderer_for_full_stream() {
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
        assert_eq!(render_stream(&events), old_render_stream(&events));
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
            name: "read".into(),
            arguments: "{\"path\": \"a.txt\"}".into(),
        }
    }
    fn agent_tool_result(ok: bool) -> AgentEvent {
        AgentEvent::ToolResult {
            name: if ok { "read" } else { "bash" }.into(),
            ok,
            result: if ok { "hello" } else { "boom" }.into(),
        }
    }
    fn agent_stop() -> AgentEvent {
        AgentEvent::Stop(StopReason::Completed)
    }
}
