//! Display-width text helpers shared by the TUI renderers: measuring display
//! width and wrapping a line to a content width, both unicode-width aware
//! (CJK and other wide characters count as two terminal columns). Everything
//! here is pure and unit-tested.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Display width of `s` in terminal columns (wide chars count two).
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Wrap `line` so it fits `width` display columns, splitting at character
/// boundaries when it would overflow. CJK and other wide characters count as
/// two columns (via `unicode-width`), matching the terminal. Returns at least
/// one row (empty input yields a single empty row) so layout stays stable.
///
/// Input wider than `width` keeps every character (nothing is dropped); a
/// zero width yields a single empty row.
pub fn wrap_to_width(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for c in line.chars() {
        let cw = c.width().unwrap_or(0);
        if current_width > 0 && current_width + cw > width {
            rows.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(c);
        current_width += cw;
    }
    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_counts_wide_chars_twice() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("a好b"), 4);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn wrap_splits_at_width_boundary() {
        assert_eq!(wrap_to_width("abcdef", 3), vec!["abc", "def"]);
        assert_eq!(wrap_to_width("abcdef", 4), vec!["abcd", "ef"]);
    }

    #[test]
    fn wrap_keeps_every_character_when_overflowing() {
        let rows = wrap_to_width(&"x".repeat(60), 10);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows.concat(), "x".repeat(60));
    }

    #[test]
    fn wrap_respects_wide_chars() {
        // 6 CJK chars = 12 columns; wrapping at width 6 needs 2 rows of 3.
        let rows = wrap_to_width("你好世界测试", 6);
        assert_eq!(rows, vec!["你好世", "界测试"]);
    }

    #[test]
    fn wrap_empty_and_zero_width() {
        assert_eq!(wrap_to_width("", 40), vec![String::new()]);
        assert_eq!(wrap_to_width("abc", 0), vec![String::new()]);
    }

    #[test]
    fn wrap_short_line_untouched() {
        assert_eq!(wrap_to_width("short", 40), vec!["short"]);
    }
}
