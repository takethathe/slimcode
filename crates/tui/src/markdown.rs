//! Markdown rendering (ADR-0006 D3, spec ticket 01): user and assistant
//! message text renders as pi-style markdown mapped onto the [`theme`]'s
//! markdown tokens. `pulldown-cmark` parses CommonMark; this module maps its
//! events to styled, width-wrapped ratatui lines. Tool output stays plain
//! text (ticket 02) — markdown is only for message text.
//!
//! Inter-block spacing belongs to the caller (`App`); this module returns one
//! wrapped line per rendered block row, in document order. Streaming re-parses
//! the merged block text per frame, so there is no incremental parser state.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::text::{display_width, wrap_to_width};
use crate::theme::{Token, fg};

/// Blockquote prefix line marker (pi's `│ ` quote border).
const QUOTE_PREFIX: &str = "│ ";
/// Unordered list bullet (pi list bullet token).
const BULLET: &str = "• ";

/// Render `text` as markdown into styled lines wrapped to `width`.
pub fn render_markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    let parser = Parser::new_ext(text, Options::empty());

    let mut out: Vec<Line<'static>> = Vec::new();
    // Inline segments of the block currently being accumulated.
    let mut segs: Vec<(String, Style)> = Vec::new();
    // Base color for the current block (heading/quote content).
    let mut block_fg: Option<Token> = None;
    // Nested emphasis/strong modifiers.
    let mut mod_stack: Vec<Modifier> = Vec::new();
    // Nested blockquotes (single `│ ` prefix for any depth).
    let mut quote_depth: usize = 0;
    // Current list: (ordered?, next number).
    let mut list: Option<(bool, u64)> = None;
    let mut in_item = false;
    // Marker of the current list item (`• ` or `N. `).
    let mut marker: Option<String> = None;
    // Inside a fenced code block: accumulate its literal text.
    let mut in_code = false;
    let mut code_text = String::new();
    let mut code_lang: Option<String> = None;
    // Inside a link: text spans take the mdLink color.
    let mut link_depth: usize = 0;

    let current_style = |block_fg: Option<Token>, mods: &[Modifier]| {
        let mut style = match block_fg {
            Some(token) => fg(token),
            None => Style::default(),
        };
        for m in mods {
            style = style.add_modifier(*m);
        }
        style
    };

    for event in parser {
        match event {
            // ---- structure starts: flush pending content first -----------
            Event::Start(Tag::Heading { .. }) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                block_fg = Some(Token::MdHeading);
            }
            Event::Start(Tag::Paragraph) => {
                if !in_item {
                    flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                    block_fg = if quote_depth > 0 {
                        Some(Token::MdQuote)
                    } else {
                        None
                    };
                }
            }
            Event::Start(Tag::BlockQuote) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                quote_depth += 1;
                block_fg = Some(Token::MdQuote);
            }
            Event::Start(Tag::List(kind)) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                marker = None;
                list = Some(match kind {
                    Some(start) => (true, start),
                    None => (false, 0),
                });
            }
            Event::Start(Tag::Item) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                in_item = true;
                match list.as_mut() {
                    Some((true, n)) => {
                        marker = Some(format!("{n}. "));
                        *n += 1;
                    }
                    _ => marker = Some(BULLET.to_string()),
                }
                if quote_depth > 0 {
                    block_fg = Some(Token::MdQuote);
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                in_code = true;
                code_text.clear();
                code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => Some(lang.to_string()),
                    pulldown_cmark::CodeBlockKind::Indented => None,
                };
            }
            Event::Start(Tag::Link { .. }) => link_depth += 1,
            Event::Start(Tag::Emphasis) => mod_stack.push(Modifier::ITALIC),
            Event::Start(Tag::Strong) => mod_stack.push(Modifier::BOLD),
            Event::Start(Tag::Strikethrough) => mod_stack.push(Modifier::CROSSED_OUT),

            // ---- inline content -------------------------------------------
            Event::Text(t) if in_code => {
                // Fenced/indented code block content is literal.
                code_text.push_str(&t);
            }
            Event::Text(t) => {
                let mut style = current_style(block_fg, &mod_stack);
                if link_depth > 0 {
                    style = style.patch(fg(Token::MdLink));
                }
                push_seg(&mut segs, &t, style);
            }
            Event::Code(t) => {
                let style = current_style(block_fg, &mod_stack).patch(fg(Token::MdCode));
                push_seg(&mut segs, &t, style);
            }
            Event::SoftBreak | Event::HardBreak => {
                // Soft and hard breaks both render as a space in the wrapped
                // paragraph (code blocks keep their own line structure).
                let style = current_style(block_fg, &mod_stack);
                push_seg(&mut segs, " ", style);
            }
            Event::Rule => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                let inner = width.saturating_sub(if quote_depth > 0 {
                    display_width(QUOTE_PREFIX)
                } else {
                    0
                });
                let mut spans = Vec::new();
                if quote_depth > 0 {
                    spans.push(Span::styled(QUOTE_PREFIX, fg(Token::MdQuoteBorder)));
                }
                spans.push(Span::styled("─".repeat(inner), fg(Token::MdHr)));
                out.push(Line::from(spans));
            }

            // ---- structure ends: flush -------------------------------------
            Event::End(TagEnd::Heading(_)) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                block_fg = None;
            }
            Event::End(TagEnd::Paragraph) => {
                if !in_item {
                    flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                    block_fg = None;
                }
            }
            Event::End(TagEnd::BlockQuote) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                quote_depth = quote_depth.saturating_sub(1);
                block_fg = None;
            }
            Event::End(TagEnd::Item) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                in_item = false;
                marker = None;
                block_fg = None;
            }
            Event::End(TagEnd::List(_)) => {
                flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
                list = None;
            }
            Event::End(TagEnd::CodeBlock) => {
                flush_code_block(
                    &mut out,
                    &code_text,
                    code_lang.as_deref(),
                    quote_depth,
                    width,
                );
                in_code = false;
                code_text.clear();
                code_lang = None;
            }
            Event::End(TagEnd::Link) => link_depth = link_depth.saturating_sub(1),
            Event::End(TagEnd::Emphasis) => pop_mod(&mut mod_stack, Modifier::ITALIC),
            Event::End(TagEnd::Strong) => pop_mod(&mut mod_stack, Modifier::BOLD),
            Event::End(TagEnd::Strikethrough) => pop_mod(&mut mod_stack, Modifier::CROSSED_OUT),

            // Html/InlineHtml/FootnoteReference/TaskListMarker/Table/... are
            // not part of the aligned subset; ignore them.
            _ => {}
        }
    }
    flush_block(&mut out, &mut segs, quote_depth, marker.as_deref(), width);
    out
}

/// Append `text` with `style` to `segs`, merging consecutive segments with
/// the same style so adjacent plain text stays one span.
fn push_seg(segs: &mut Vec<(String, Style)>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = segs.last_mut()
        && last.1 == style
    {
        last.0.push_str(text);
        return;
    }
    segs.push((text.to_string(), style));
}

/// Pop the first occurrence of `m` from the modifier stack (nested emphasis
/// inside strong, etc.).
fn pop_mod(stack: &mut Vec<Modifier>, m: Modifier) {
    if let Some(pos) = stack.iter().rposition(|x| *x == m) {
        stack.remove(pos);
    }
}

/// Wrap the style-aware `segs` into lines of styled spans, breaking at
/// character boundaries (wide chars count two), keeping each style segment
/// contiguous and merging adjacent same-style spans. `width == 0` disables
/// wrapping (single line).
fn wrap_segs(segs: &[(String, Style)], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut line_widths = vec![0usize];
    for (text, style) in segs {
        let mut rest = text.as_str();
        while !rest.is_empty() {
            let used = *line_widths.last().unwrap();
            if width > 0 && used > 0 && used >= width {
                lines.push(Vec::new());
                line_widths.push(0);
                continue;
            }
            let mut chunk = String::new();
            let mut chunk_w = 0usize;
            let mut consumed = 0usize;
            let mut next_line = false;
            for (i, c) in rest.char_indices() {
                let cw = c.width().unwrap_or(0);
                consumed = i + c.len_utf8();
                if !chunk.is_empty() && chunk_w + cw > width {
                    // Prefer a word boundary: cut at the last space so words
                    // stay whole; the overflow rest starts the next line
                    // (CJK and overlong words fall back to a character break).
                    if let Some(pos) = chunk.rfind(' ') {
                        consumed = pos + 1;
                        chunk.truncate(pos);
                        chunk_w = display_width(&chunk);
                        next_line = true;
                    }
                    break;
                }
                chunk.push(c);
                chunk_w += cw;
            }
            let spans = lines.last_mut().unwrap();
            if let Some(last) = spans.last_mut()
                && last.style == *style
            {
                last.content.to_mut().push_str(&chunk);
            } else {
                spans.push(Span::styled(chunk, *style));
            }
            *line_widths.last_mut().unwrap() += chunk_w;
            rest = &rest[consumed..];
            if next_line {
                // The overflow rest continues on a fresh line.
                lines.push(Vec::new());
                line_widths.push(0);
            }
        }
    }
    lines
}

/// Flush the accumulated inline segments as wrapped lines, prefixing
/// blockquote lines with `│ ` and the first line with the list marker.
fn flush_block(
    out: &mut Vec<Line<'static>>,
    segs: &mut Vec<(String, Style)>,
    quote_depth: usize,
    marker: Option<&str>,
    total_width: usize,
) {
    if segs.is_empty() {
        return;
    }
    let quote_w = if quote_depth > 0 {
        display_width(QUOTE_PREFIX)
    } else {
        0
    };
    let mark_w = marker.map(display_width).unwrap_or(0);
    let inner = total_width.saturating_sub(quote_w + mark_w);
    let lines = wrap_segs(segs, inner);
    for (i, spans) in lines.into_iter().enumerate() {
        let mut final_spans: Vec<Span<'static>> = Vec::new();
        if quote_depth > 0 {
            final_spans.push(Span::styled(QUOTE_PREFIX, fg(Token::MdQuoteBorder)));
        }
        if i == 0
            && let Some(m) = marker
        {
            final_spans.push(Span::styled(m.to_string(), fg(Token::MdListBullet)));
        }
        final_spans.extend(spans);
        out.push(Line::from(final_spans));
    }
    segs.clear();
}

/// Push the accumulated fenced/indented code block text following pi's
/// layout: a `` ``` `` border line (mdCodeBlockBorder, gray), each code line
/// indented two spaces (mdCodeBlock, green), and a closing border line.
fn flush_code_block(
    out: &mut Vec<Line<'static>>,
    code_text: &str,
    code_lang: Option<&str>,
    quote_depth: usize,
    total_width: usize,
) {
    let quote_w = if quote_depth > 0 {
        display_width(QUOTE_PREFIX)
    } else {
        0
    };
    let inner = total_width.saturating_sub(quote_w);
    let fence = match code_lang {
        Some(lang) if !lang.is_empty() => format!("```{lang}"),
        _ => "```".to_string(),
    };
    let lines = std::iter::once(fence)
        .chain(code_text.trim_end_matches('\n').split('\n').map(|l| {
            if l.is_empty() {
                l.to_string()
            } else {
                format!("  {l}")
            }
        }))
        .chain(std::iter::once("```".to_string()));
    for line in lines {
        for row in wrap_to_width(&line, inner) {
            let mut spans = Vec::new();
            if quote_depth > 0 {
                spans.push(Span::styled(QUOTE_PREFIX, fg(Token::MdQuoteBorder)));
            }
            if row == "```" || row.starts_with("```") {
                spans.push(Span::styled(row, fg(Token::MdCodeBlockBorder)));
            } else {
                spans.push(Span::styled(row, fg(Token::MdCodeBlock)));
            }
            out.push(Line::from(spans));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.to_string())
            .collect::<String>()
    }

    fn all_fg(line: &Line<'static>) -> Option<Color> {
        let mut fg: Option<Color> = None;
        for s in line.spans.iter() {
            if fg.is_none() {
                fg = s.style.fg;
            } else if s.style.fg != fg {
                return None; // mixed colors
            }
        }
        fg
    }

    fn span_with<'a>(line: &'a Line<'static>, needle: &str) -> Option<&'a Span<'static>> {
        line.spans.iter().find(|s| s.content.contains(needle))
    }

    #[test]
    fn heading_renders_in_md_heading_color_without_markers() {
        let lines = render_markdown("# Title", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "Title");
        assert_eq!(all_fg(&lines[0]), Some(Token::MdHeading.color()));
    }

    #[test]
    fn bold_and_italic_carry_modifiers_and_combine_text() {
        let text = "**bold** and *italic*";
        let lines = render_markdown(text, 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "bold and italic");
        let bold = span_with(&lines[0], "bold").unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let italic = span_with(&lines[0], "italic").unwrap();
        assert!(italic.style.add_modifier.contains(Modifier::ITALIC));
        let plain = span_with(&lines[0], " and ").unwrap();
        assert!(!plain.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn inline_code_uses_md_code_color() {
        let lines = render_markdown("use `let` here", 80);
        assert_eq!(line_text(&lines[0]), "use let here");
        let code = span_with(&lines[0], "let").unwrap();
        assert_eq!(code.style.fg, Some(Token::MdCode.color()));
    }

    #[test]
    fn fenced_code_block_renders_fence_borders_and_indented_lines() {
        let md = "```rust\nlet x = 1;\nlet y = 2;\n```";
        let lines = render_markdown(md, 80);
        assert_eq!(lines.len(), 4);
        assert_eq!(line_text(&lines[0]), "```rust");
        assert_eq!(all_fg(&lines[0]), Some(Token::MdCodeBlockBorder.color()));
        assert_eq!(line_text(&lines[1]), "  let x = 1;");
        assert_eq!(all_fg(&lines[1]), Some(Token::MdCodeBlock.color()));
        assert_eq!(line_text(&lines[2]), "  let y = 2;");
        assert_eq!(all_fg(&lines[2]), Some(Token::MdCodeBlock.color()));
        assert_eq!(line_text(&lines[3]), "```");
        assert_eq!(all_fg(&lines[3]), Some(Token::MdCodeBlockBorder.color()));
    }

    #[test]
    fn fenced_code_without_lang_uses_bare_fence() {
        let lines = render_markdown("```\ncode\n```", 80);
        assert_eq!(line_text(&lines[0]), "```");
        assert_eq!(line_text(&lines[1]), "  code");
        assert_eq!(line_text(&lines[2]), "```");
    }

    #[test]
    fn indented_code_block_uses_bare_fence() {
        let lines = render_markdown("    indented\n    code", 80);
        assert_eq!(line_text(&lines[0]), "```");
        assert_eq!(line_text(&lines[1]), "  indented");
        assert_eq!(line_text(&lines[2]), "  code");
        assert_eq!(line_text(&lines[3]), "```");
    }

    #[test]
    fn blockquote_prefixes_every_line_in_quote_colors() {
        // Two adjacent `> ` lines form one quote paragraph (soft break); a
        // blank line starts a second blockquote.
        let lines = render_markdown("> quoted one\n> quoted two\n\n> second quote", 80);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "│ quoted one quoted two");
        assert_eq!(line_text(&lines[1]), "│ second quote");
        let prefix = span_with(&lines[0], "│").unwrap();
        assert_eq!(prefix.style.fg, Some(Token::MdQuoteBorder.color()));
        let body = span_with(&lines[0], "quoted").unwrap();
        assert_eq!(body.style.fg, Some(Token::MdQuote.color()));
    }

    #[test]
    fn unordered_list_renders_bullets_with_md_list_color() {
        let lines = render_markdown("- alpha\n- beta", 80);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "• alpha");
        assert_eq!(line_text(&lines[1]), "• beta");
        let bullet = span_with(&lines[0], "•").unwrap();
        assert_eq!(bullet.style.fg, Some(Token::MdListBullet.color()));
    }

    #[test]
    fn ordered_list_numbers_increment() {
        let lines = render_markdown("1. one\n2. two", 80);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "1. one");
        assert_eq!(line_text(&lines[1]), "2. two");
    }

    #[test]
    fn ordered_list_honors_explicit_start() {
        let lines = render_markdown("5. five", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "5. five");
    }

    #[test]
    fn link_text_uses_md_link_color() {
        let lines = render_markdown("[pi](https://pi.dev)", 80);
        assert_eq!(line_text(&lines[0]), "pi");
        let link = span_with(&lines[0], "pi").unwrap();
        assert_eq!(link.style.fg, Some(Token::MdLink.color()));
    }

    #[test]
    fn rule_renders_full_width_dash_line() {
        let lines = render_markdown("---", 40);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "─".repeat(40));
        assert_eq!(all_fg(&lines[0]), Some(Token::MdHr.color()));
    }

    #[test]
    fn long_paragraph_wraps_to_width_keeping_all_text() {
        let words: Vec<String> = (0..30).map(|i| format!("word{i}")).collect();
        let md = words.join(" ");
        let lines = render_markdown(&md, 20);
        assert!(lines.len() > 1, "long paragraph should wrap");
        for line in &lines {
            assert!(
                display_width(&line_text(line)) <= 20,
                "line too wide: {line:?}"
            );
        }
        // Word order is preserved across the wrapped lines (single spaces).
        let joined = lines.iter().map(line_text).collect::<Vec<_>>().join(" ");
        let normalized = joined.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(normalized, md);
    }

    #[test]
    fn mixed_document_renders_blocks_in_order() {
        let md = "# Heading\n\nplain *em* text\n\n- one\n- two\n\n> quoted\n\n```\ncode\n```";
        let lines = render_markdown(md, 200);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts[0], "Heading");
        assert_eq!(all_fg(&lines[0]), Some(Token::MdHeading.color()));
        assert_eq!(texts[1], "plain em text");
        assert_eq!(texts[2], "• one");
        assert_eq!(texts[3], "• two");
        assert_eq!(texts[4], "│ quoted");
        assert_eq!(texts[5], "```");
        assert_eq!(all_fg(&lines[5]), Some(Token::MdCodeBlockBorder.color()));
        assert_eq!(texts[6], "  code");
        assert_eq!(all_fg(&lines[6]), Some(Token::MdCodeBlock.color()));
        assert_eq!(texts[7], "```");
        assert_eq!(all_fg(&lines[7]), Some(Token::MdCodeBlockBorder.color()));
    }

    #[test]
    fn nested_bold_italic_keeps_both_modifiers() {
        let lines = render_markdown("**_both_**", 80);
        let span = span_with(&lines[0], "both").unwrap();
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }
}
