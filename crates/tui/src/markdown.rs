//! Markdown rendering (ADR-0006 D3, spec ticket 01): user and assistant
//! message text renders as pi-style markdown mapped onto the [`theme`]'s
//! markdown tokens. `pulldown-cmark` parses CommonMark; this module maps its
//! events to styled, width-wrapped ratatui lines. Tool output stays plain
//! text (ticket 02) — markdown is only for message text.
//!
//! Two levels own spacing, and this module owns exactly one of them. *Inside* a
//! markdown document the renderer separates adjacent blocks with one gap row
//! (blank, or `│ ` inside a blockquote); a run of source blank lines collapses
//! to that single row because `pulldown-cmark` emits no event for a blank line,
//! only for the blocks around it. *Between* transcript blocks (a markdown
//! document is only one such block) the spacing belongs to `App`'s row builder.
//! Streaming re-parses the merged block text per frame, so there is no
//! incremental parser state.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::text::{display_width, wrap_to_width};
use crate::theme::{Token, fg};

/// Blockquote prefix line marker (pi's `│ ` quote border).
const QUOTE_PREFIX: &str = "│ ";
/// Unordered list bullet. pi normalizes `*` and `+` to `- `, and so do we.
const BULLET: &str = "- ";
/// Columns of indentation per list nesting level (pi's `renderList`).
const LIST_INDENT: usize = 4;

/// Append a gap row (one blank row separating two markdown blocks), unless the
/// output is empty (a document never starts with a gap) or already ends with
/// one. The idempotency is what keeps "exactly one gap row" true on every
/// path: a block whose own gap meets the next block's gap pushes nothing the
/// second time. Inside a blockquote the gap carries only the quote border, so
/// the `│ ` edge stays continuous.
fn push_gap(out: &mut Vec<Line<'static>>, quote_depth: usize) {
    if out.is_empty() || out.last().is_some_and(is_gap_row) {
        return;
    }
    if quote_depth > 0 {
        out.push(Line::from(Span::styled(
            QUOTE_PREFIX,
            fg(Token::MdQuoteBorder),
        )));
    } else {
        out.push(Line::default());
    }
}

/// Whether `line` is a gap row: blank, or carrying only the quote border.
fn is_gap_row(line: &Line<'static>) -> bool {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    text.is_empty() || text == QUOTE_PREFIX
}

/// One open list: ordered-ness, the next ordered number, and its nesting depth
/// (0 for a top-level list). A list is a stack frame rather than a single slot
/// so a nested list keeps its own counter and the outer list is restored when
/// the nested one closes.
struct ListFrame {
    ordered: bool,
    next: u64,
    depth: usize,
}

/// The item currently being rendered: its marker, the indentation before the
/// marker, and whether it has emitted a row yet. The first emitted row carries
/// the marker (the item's "bullet row"); later rows line up under the item text.
struct ItemFrame {
    /// Marker text for the item's first row: `- ` for an unordered item (pi
    /// normalizes `*` / `+` to `- `) or `N. ` for an ordered one.
    marker: String,
    /// Columns of indentation before the marker.
    indent: usize,
    /// Whether a row has been emitted for this item already.
    rendered: bool,
    /// Whether the item is paragraph-wrapped. `pulldown-cmark` emits paragraph
    /// events for a loose list's items (and for any item holding two blocks)
    /// and plain text for a tight list's, so this is the CommonMark signal of a
    /// loose list — the only case that gets a gap row after it.
    loose: bool,
}

impl ItemFrame {
    /// The prefix of the item's first row: indentation followed by the marker.
    fn first_prefix(&self) -> String {
        format!("{}{}", " ".repeat(self.indent), self.marker)
    }

    /// The prefix of the item's continuation rows (wrapped text, a second
    /// paragraph, a code block): as wide as the first prefix, so every row
    /// lines up under the item text.
    fn continuation_prefix(&self) -> String {
        " ".repeat(self.indent + display_width(&self.marker))
    }

    /// Display width of [`ItemFrame::first_prefix`].
    fn first_width(&self) -> usize {
        self.indent + display_width(&self.marker)
    }

    /// Display width of [`ItemFrame::continuation_prefix`].
    fn continuation_width(&self) -> usize {
        display_width(&self.continuation_prefix())
    }
}

/// The list state: a stack of open lists plus the stack of open items (an
/// item is pushed when its list item opens and popped when it closes, so the
/// outer item is current again after a nested list ends).
#[derive(Default)]
struct Lists {
    frames: Vec<ListFrame>,
    items: Vec<ItemFrame>,
}

impl Lists {
    /// The current (innermost) item, if any.
    fn item(&self) -> Option<&ItemFrame> {
        self.items.last()
    }

    /// The current (innermost) item, mutably.
    fn item_mut(&mut self) -> Option<&mut ItemFrame> {
        self.items.last_mut()
    }

    /// Whether rendering is currently inside a list item.
    fn in_item(&self) -> bool {
        !self.items.is_empty()
    }

    /// Record that the current item has emitted a row, so its later rows use
    /// the continuation prefix.
    fn mark_rendered(&mut self) {
        if let Some(item) = self.items.last_mut() {
            item.rendered = true;
        }
    }
}

/// The leading spans of one rendered row: the blockquote border (present for
/// any quote depth) followed by the current item's first-row or continuation
/// prefix. This is the one pipeline every row goes through, so the quote and
/// item prefixes can never drift apart across block kinds.
fn prefix_spans(quote_depth: usize, item: Option<&ItemFrame>, first: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if quote_depth > 0 {
        spans.push(Span::styled(QUOTE_PREFIX, fg(Token::MdQuoteBorder)));
    }
    if let Some(item) = item {
        if first {
            spans.push(Span::styled(item.first_prefix(), fg(Token::MdListBullet)));
        } else {
            let prefix = item.continuation_prefix();
            if !prefix.is_empty() {
                spans.push(Span::raw(prefix));
            }
        }
    }
    spans
}

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
    // Open lists and items (see [`Lists`]).
    let mut lists = Lists::default();
    // Set when an item ends that was paragraph-wrapped (loose list): the next
    // item's row is separated by one gap row. Cleared at the list's end, so no
    // gap ever trails the last item.
    let mut loose_gap_pending = false;
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
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                if !lists.in_item() {
                    push_gap(&mut out, quote_depth);
                }
                block_fg = Some(Token::MdHeading);
            }
            Event::Start(Tag::Paragraph) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                if lists.in_item() {
                    // A paragraph inside an item is the CommonMark loose-list
                    // signal; its text is flushed as its own row block, so an
                    // item holding two paragraphs renders two rows.
                    if let Some(item) = lists.item_mut() {
                        item.loose = true;
                    }
                } else {
                    push_gap(&mut out, quote_depth);
                }
                block_fg = if quote_depth > 0 {
                    Some(Token::MdQuote)
                } else {
                    None
                };
            }
            Event::Start(Tag::BlockQuote) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                if !lists.in_item() {
                    push_gap(&mut out, quote_depth);
                }
                quote_depth += 1;
                block_fg = Some(Token::MdQuote);
            }
            Event::Start(Tag::List(kind)) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                if !lists.in_item() {
                    push_gap(&mut out, quote_depth);
                }
                // A nested list is content of the item it sits in: that item
                // has now emitted rows, so its later blocks use the
                // continuation prefix.
                if let Some(item) = lists.item_mut() {
                    item.rendered = true;
                }
                let depth = lists.frames.len();
                let (ordered, next) = match kind {
                    Some(start) => (true, start),
                    None => (false, 0),
                };
                lists.frames.push(ListFrame {
                    ordered,
                    next,
                    depth,
                });
            }
            Event::Start(Tag::Item) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                if loose_gap_pending {
                    push_gap(&mut out, quote_depth);
                    loose_gap_pending = false;
                }
                let (marker, depth) = match lists.frames.last_mut() {
                    Some(frame) if frame.ordered => {
                        let marker = format!("{}. ", frame.next);
                        frame.next += 1;
                        (marker, frame.depth)
                    }
                    Some(frame) => (BULLET.to_string(), frame.depth),
                    // An item event without an open list is malformed; treat it
                    // as a top-level bullet so rendering stays total.
                    None => (BULLET.to_string(), 0),
                };
                lists.items.push(ItemFrame {
                    marker,
                    indent: depth * LIST_INDENT,
                    rendered: false,
                    loose: false,
                });
                if quote_depth > 0 {
                    block_fg = Some(Token::MdQuote);
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                if !lists.in_item() {
                    push_gap(&mut out, quote_depth);
                }
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
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                if !lists.in_item() {
                    push_gap(&mut out, quote_depth);
                }
                // The rule is a row like any other: it goes through the same
                // prefix pipeline and reserves the item prefix width, so an
                // indented item's rule stays inside the pane.
                let item_w = lists.item().map(ItemFrame::first_width).unwrap_or(0);
                let first = lists.item().map(|item| !item.rendered).unwrap_or(false);
                let quote_w = if quote_depth > 0 {
                    display_width(QUOTE_PREFIX)
                } else {
                    0
                };
                let inner = width.saturating_sub(quote_w + item_w);
                let mut spans = prefix_spans(quote_depth, lists.item(), first);
                spans.push(Span::styled("─".repeat(inner), fg(Token::MdHr)));
                out.push(Line::from(spans));
                lists.mark_rendered();
            }

            // ---- structure ends: flush -------------------------------------
            Event::End(TagEnd::Heading(_)) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                block_fg = None;
            }
            Event::End(TagEnd::Paragraph) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                block_fg = None;
            }
            Event::End(TagEnd::BlockQuote) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                quote_depth = quote_depth.saturating_sub(1);
                block_fg = None;
            }
            Event::End(TagEnd::Item) => {
                let loose = lists.item_mut().map(|item| item.loose).unwrap_or(false);
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                lists.items.pop();
                loose_gap_pending = loose;
                block_fg = None;
            }
            Event::End(TagEnd::List(_)) => {
                flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
                lists.frames.pop();
                // No gap after the last item: the pending loose gap only ever
                // fires before a following item.
                loose_gap_pending = false;
            }
            Event::End(TagEnd::CodeBlock) => {
                flush_code_block(
                    &mut out,
                    &code_text,
                    code_lang.as_deref(),
                    quote_depth,
                    lists.item_mut(),
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
    flush_block(&mut out, &mut segs, quote_depth, lists.item_mut(), width);
    // Never hand the caller a document that ends in a blank row: the trailing
    // gap would show up as dead space above the dock.
    while out.last().is_some_and(is_gap_row) {
        out.pop();
    }
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
                if width > 0 && !chunk.is_empty() && chunk_w + cw > width {
                    // Prefer a word boundary: cut at the last space so words
                    // stay whole and the space is consumed at the break. With
                    // no space in the chunk, break *before* the overflowing
                    // character so it starts the next row and is never lost
                    // (CJK and overlong words fall back to a character break).
                    if let Some(pos) = chunk.rfind(' ') {
                        consumed = pos + 1;
                        chunk.truncate(pos);
                        chunk_w = display_width(&chunk);
                        next_line = true;
                    } else {
                        consumed = i;
                    }
                    break;
                }
                chunk.push(c);
                chunk_w += cw;
                consumed = i + c.len_utf8();
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

/// Flush the accumulated inline segments as wrapped lines. Every row is
/// prefixed through [`prefix_spans`]: the blockquote border always, and the
/// current item's first-row prefix for the item's first row, the continuation
/// prefix for the rest. Wrapping reserves the first-row prefix width (pi's
/// `itemWidth`), so a list item can never overflow the pane.
fn flush_block(
    out: &mut Vec<Line<'static>>,
    segs: &mut Vec<(String, Style)>,
    quote_depth: usize,
    item: Option<&mut ItemFrame>,
    total_width: usize,
) {
    if segs.is_empty() {
        return;
    }
    let item_width = item.as_deref().map(ItemFrame::first_width).unwrap_or(0);
    let quote_w = if quote_depth > 0 {
        display_width(QUOTE_PREFIX)
    } else {
        0
    };
    let inner = total_width.saturating_sub(quote_w + item_width);
    let lines = wrap_segs(segs, inner);
    let mut first = item.as_deref().map(|i| !i.rendered).unwrap_or(false);
    for spans in lines {
        let mut final_spans = prefix_spans(quote_depth, item.as_deref(), first);
        final_spans.extend(spans);
        out.push(Line::from(final_spans));
        first = false;
    }
    if let Some(item) = item {
        item.rendered = true;
    }
    segs.clear();
}

/// Push the accumulated fenced/indented code block text following pi's
/// layout: a `` ``` `` border line (mdCodeBlockBorder, gray), each code line
/// indented two spaces (mdCodeBlock, green), and a closing border line. Rows
/// go through the same prefix pipeline as text; a code block inside an item is
/// a continuation block, so it lines up under the item text.
fn flush_code_block(
    out: &mut Vec<Line<'static>>,
    code_text: &str,
    code_lang: Option<&str>,
    quote_depth: usize,
    item: Option<&mut ItemFrame>,
    total_width: usize,
) {
    let quote_w = if quote_depth > 0 {
        display_width(QUOTE_PREFIX)
    } else {
        0
    };
    let item_w = item
        .as_deref()
        .map(ItemFrame::continuation_width)
        .unwrap_or(0);
    let inner = total_width.saturating_sub(quote_w + item_w);
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
            let mut spans = prefix_spans(quote_depth, item.as_deref(), false);
            if row == "```" || row.starts_with("```") {
                spans.push(Span::styled(row, fg(Token::MdCodeBlockBorder)));
            } else {
                spans.push(Span::styled(row, fg(Token::MdCodeBlock)));
            }
            out.push(Line::from(spans));
        }
    }
    if let Some(item) = item {
        item.rendered = true;
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
        // blank line starts a second blockquote, separated by one plain gap.
        let lines = render_markdown("> quoted one\n> quoted two\n\n> second quote", 80);
        assert_eq!(lines.len(), 3);
        assert_eq!(line_text(&lines[0]), "│ quoted one quoted two");
        assert_eq!(line_text(&lines[1]), "");
        assert_eq!(line_text(&lines[2]), "│ second quote");
        let prefix = span_with(&lines[0], "│").unwrap();
        assert_eq!(prefix.style.fg, Some(Token::MdQuoteBorder.color()));
        let body = span_with(&lines[0], "quoted").unwrap();
        assert_eq!(body.style.fg, Some(Token::MdQuote.color()));
    }

    #[test]
    fn heading_is_followed_by_a_gap_row_before_its_body() {
        let lines = render_markdown("# Title\n\nbody", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["Title", "", "body"]);
        // The gap is a plain unstyled blank row.
        assert!(lines[1].spans.is_empty());
    }

    #[test]
    fn two_paragraphs_are_separated_by_one_gap_row() {
        let lines = render_markdown("first\n\nsecond", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["first", "", "second"]);
        assert!(lines[1].spans.is_empty());
    }

    #[test]
    fn a_run_of_blank_lines_collapses_to_one_gap_row() {
        let lines = render_markdown("first\n\n\n\n\nsecond", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["first", "", "second"]);
    }

    #[test]
    fn heading_directly_after_a_paragraph_still_gets_a_gap() {
        let lines = render_markdown("para\n# Title", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["para", "", "Title"]);
    }

    #[test]
    fn code_block_is_fenced_by_gap_rows() {
        let lines = render_markdown("before\n\n```\ncode\n```\n\nafter", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["before", "", "```", "  code", "```", "", "after"]);
    }

    #[test]
    fn horizontal_rule_is_fenced_by_gap_rows() {
        let lines = render_markdown("before\n\n---\n\nafter", 20);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["before", "", "─".repeat(20).as_str(), "", "after"]);
    }

    #[test]
    fn blank_line_inside_a_blockquote_is_a_quote_gap_row() {
        let lines = render_markdown("> a\n>\n> b", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["│ a", "│ ", "│ b"]);
        // The quote gap carries only the quote border, in the border color.
        assert_eq!(lines[1].spans.len(), 1);
        assert_eq!(lines[1].spans[0].content, "│ ");
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(Token::MdQuoteBorder.color())
        );
    }

    #[test]
    fn document_ending_in_blank_lines_has_no_trailing_gap_row() {
        let lines = render_markdown("text\n\n\n", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["text"]);
    }

    // --- list geometry (ticket 04) -----------------------------------------

    #[test]
    fn nested_list_indents_four_spaces_per_level() {
        let lines = render_markdown("- a\n  - b\n    - c", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- a", "    - b", "        - c"]);
    }

    #[test]
    fn loose_list_separates_items_with_one_gap_row_and_no_trailing_gap() {
        let lines = render_markdown("- a\n\n- b", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- a", "", "- b"]);
        // The gap row is plain and unstyled.
        assert!(lines[1].spans.is_empty());

        // A block after a loose list gets exactly one gap too (the loose gap
        // and the block gap must not double up).
        let lines = render_markdown("- a\n\n- b\n\nafter", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- a", "", "- b", "", "after"]);
    }

    #[test]
    fn item_with_two_paragraphs_gives_each_its_own_row() {
        let lines = render_markdown("- a1\n\n  a2", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- a1", "  a2"]);
    }

    #[test]
    fn blocks_inside_a_list_item_stay_adjacent_without_a_gap_row() {
        // The item holds a paragraph and a code block; within the item the two
        // blocks are adjacent rows (no gap), and the code block lines up under
        // the item text.
        let lines = render_markdown("- a\n\n  ```\n  code\n  ```", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- a", "  ```", "    code", "  ```"]);
    }

    #[test]
    fn unordered_list_renders_dash_bullets_with_md_list_color() {
        let lines = render_markdown("- alpha\n- beta", 80);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "- alpha");
        assert_eq!(line_text(&lines[1]), "- beta");
        let bullet = span_with(&lines[0], "- ").unwrap();
        assert_eq!(bullet.style.fg, Some(Token::MdListBullet.color()));
    }

    #[test]
    fn star_and_plus_bullets_normalize_to_dash() {
        let lines = render_markdown("* alpha\n* beta", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- alpha", "- beta"]);
        let lines = render_markdown("+ gamma", 80);
        assert_eq!(line_text(&lines[0]), "- gamma");
    }

    #[test]
    fn checkbox_spellings_are_preserved_verbatim_in_body_color() {
        let lines = render_markdown("- [x] done\n- [ ] todo\n- [X] upper", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- [x] done", "- [ ] todo", "- [X] upper"]);
        // The checkbox is ordinary item text: body color, never repainted as
        // the bullet decoration.
        let checkbox = span_with(&lines[0], "[x]").unwrap();
        assert_eq!(checkbox.style.fg, None);
        assert_eq!(checkbox.style.add_modifier, Modifier::empty());
        let bullet = span_with(&lines[0], "- ").unwrap();
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
    fn nested_ordered_list_inside_unordered_keeps_numbering_and_indent() {
        let lines = render_markdown("- a\n  1. one\n  2. two", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- a", "    1. one", "    2. two"]);
    }

    #[test]
    fn nested_unordered_list_inside_ordered_keeps_bullet_and_indent() {
        let lines = render_markdown("1. a\n   - b", 80);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["1. a", "    - b"]);
    }

    #[test]
    fn wrapped_item_continuation_rows_align_under_the_item_text() {
        let lines = render_markdown("- alpha beta gamma delta epsilon zeta", 24);
        assert!(lines.len() > 1, "the item should wrap: {lines:#?}");
        assert!(line_text(&lines[0]).starts_with("- "));
        for line in &lines[1..] {
            let text = line_text(line);
            assert!(text.starts_with("  "), "continuation not aligned: {text:?}");
            assert!(
                !text.starts_with("  - "),
                "continuation re-used bullet: {text:?}"
            );
        }
        for line in &lines {
            assert!(display_width(&line_text(line)) <= 24, "overflow: {line:?}");
        }
    }

    #[test]
    fn no_row_overflows_the_content_width_at_a_nested_level() {
        let lines = render_markdown(
            "- outer text that wraps around\n  - nested text that wraps too",
            20,
        );
        for line in &lines {
            assert!(
                display_width(&line_text(line)) <= 20,
                "row overflows: {:?}",
                line_text(line)
            );
        }
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
    fn horizontal_rule_inside_a_list_item_is_indented_with_the_item() {
        let lines = render_markdown("- a\n\n  ---", 20);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts, ["- a", format!("  {}", "─".repeat(18)).as_str()]);
        assert!(display_width(&texts[1]) <= 20);
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
    fn unbroken_text_wraps_without_dropping_characters() {
        // No space to break on: the overflowing character must start the next
        // row, never be sliced away.
        let md = "a".repeat(17);
        let lines = render_markdown(&md, 4);
        assert!(lines.len() > 1, "should wrap: {lines:#?}");
        let joined: String = lines.iter().map(line_text).collect();
        assert_eq!(joined, md);
        for line in &lines {
            assert!(display_width(&line_text(line)) <= 4, "overflow: {line:?}");
        }
    }

    #[test]
    fn zero_width_renders_one_unwrapped_line() {
        let lines = render_markdown("hello world", 0);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "hello world");
    }

    #[test]
    fn mixed_document_renders_blocks_in_order() {
        let md = "# Heading\n\nplain *em* text\n\n- one\n- two\n\n> quoted\n\n```\ncode\n```";
        let lines = render_markdown(md, 200);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(texts[0], "Heading");
        assert_eq!(all_fg(&lines[0]), Some(Token::MdHeading.color()));
        assert_eq!(texts[1], "");
        assert_eq!(texts[2], "plain em text");
        assert_eq!(texts[3], "");
        assert_eq!(texts[4], "- one");
        assert_eq!(texts[5], "- two");
        assert_eq!(texts[6], "");
        assert_eq!(texts[7], "│ quoted");
        assert_eq!(texts[8], "");
        assert_eq!(texts[9], "```");
        assert_eq!(all_fg(&lines[9]), Some(Token::MdCodeBlockBorder.color()));
        assert_eq!(texts[10], "  code");
        assert_eq!(all_fg(&lines[10]), Some(Token::MdCodeBlock.color()));
        assert_eq!(texts[11], "```");
        assert_eq!(all_fg(&lines[11]), Some(Token::MdCodeBlockBorder.color()));
    }

    #[test]
    fn nested_bold_italic_keeps_both_modifiers() {
        let lines = render_markdown("**_both_**", 80);
        let span = span_with(&lines[0], "both").unwrap();
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }
}
