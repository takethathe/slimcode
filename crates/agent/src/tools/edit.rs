//! `edit` tool engine: exact-text replacement with unique-match enforcement and
//! a lightweight fuzzy fallback.
//!
//! Semantics locked in `.scratch/slimcode-v1` ticket 03:
//!
//! 1. Input `{ path, edits: [{oldText, newText}] }`; one call may carry several
//!    **disjoint** edits.
//! 2. Every edit is matched against the **original** file (not incrementally);
//!    replacements apply in reverse offset order so offsets stay stable.
//! 3. Each `oldText` must occur **exactly once**; 0 → not-found, >1 → not-unique.
//! 4. Matches must not **overlap**.
//! 5. Empty `oldText` is an error.
//! 6. A no-op replacement (identical content) is an error.
//! 7. Line endings (CRLF/LF) are detected, normalized to LF for matching, then
//!    restored; a leading BOM is stripped before matching and restored after.
//! 8. Fuzzy fallback is **lightweight (option C)**: exact match first; on miss,
//!    retry after per-line `trim_end` normalization only (no NFKC / Unicode
//!    quote/dash folding). When a fuzzy match is used, matched line blocks are
//!    rewritten from the normalized view while untouched lines keep their
//!    original bytes.
//!
//! The engine is pure (no file I/O); file access lives with the tool binding.

/// One targeted replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

impl Edit {
    pub fn new(old_text: impl Into<String>, new_text: impl Into<String>) -> Self {
        Self {
            old_text: old_text.into(),
            new_text: new_text.into(),
        }
    }
}

/// Result of a successful apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEdit {
    pub new_content: String,
    /// Number of replaced blocks (one per successfully matched edit).
    pub replaced_blocks: usize,
    /// Display-oriented line diff (line-numbered `+`/`-`/context lines).
    pub diff: String,
    /// 1-based line number of the first change in the new content.
    pub first_changed_line: Option<usize>,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

/// Detect the dominant line ending (first `\r\n` vs first `\n`).
fn detect_line_ending(content: &str) -> &'static str {
    match (content.find("\r\n"), content.find('\n')) {
        (Some(c), Some(l)) if c < l => "\r\n",
        _ => "\n",
    }
}

fn normalize_to_lf(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_line_endings(content: &str, ending: &'static str) -> String {
    if ending == "\r\n" {
        content.replace('\n', "\r\n")
    } else {
        content.to_string()
    }
}

fn split_bom(content: &str) -> (bool, &str) {
    let has = content.starts_with('\u{feff}');
    (has, content.strip_prefix('\u{feff}').unwrap_or(content))
}

/// Lightweight fuzzy normalization (option C): strip trailing whitespace per line.
/// No NFKC / Unicode quote/dash folding.
fn trim_end_lines(content: &str) -> String {
    content
        .split('\n')
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Match {
    edit_index: usize,
    match_index: usize,
    match_len: usize,
    new_text: String,
}

/// Split into line spans, each `(start, end)` covering the line content WITHOUT
/// its trailing `\n`.
fn line_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for part in content.split('\n') {
        let end = start + part.len();
        spans.push((start, end));
        start = end + 1; // skip the '\n'
    }
    spans
}

/// Index of the line containing `offset` (0-based). Spans cover line content
/// WITHOUT their trailing `\n`, so an offset pointing at a line's `\n` (span
/// end) is attributed to the line that terminates there.
fn line_of_offset(spans: &[(usize, usize)], offset: usize) -> usize {
    spans
        .iter()
        .position(|(s, e)| offset >= *s && offset <= *e)
        .unwrap_or(spans.len().saturating_sub(1))
}

/// Apply `matches` (offsets relative to `base_offset` in `content`) in reverse
/// offset order so earlier offsets stay stable.
fn apply_replacements(content: &mut String, mut matches: Vec<Match>, base_offset: usize) {
    matches.sort_by_key(|m| std::cmp::Reverse(m.match_index));
    for m in &matches {
        let local = m.match_index - base_offset;
        content.replace_range(local..local + m.match_len, &m.new_text);
    }
}

fn apply_direct(content: &str, matches: &[Match]) -> String {
    let mut result = content.to_string();
    apply_replacements(&mut result, matches.to_vec(), 0);
    result
}

/// Rewrite matched line blocks from the normalized (`base`) view while copying
/// untouched lines verbatim from `original`. `matches` are offsets into `base`.
fn apply_preserving(original: &str, base: &str, matches: &[Match]) -> String {
    let spans = line_spans(base);
    // group matches into contiguous line ranges
    let mut groups: Vec<(usize, usize, Vec<Match>)> = Vec::new(); // (start_line, end_line, matches)
    let mut sorted = matches.to_vec();
    sorted.sort_by_key(|m| m.match_index);
    for m in &sorted {
        let s = line_of_offset(&spans, m.match_index);
        let last = m.match_index + m.match_len - 1;
        let e = line_of_offset(&spans, last);
        if let Some((_, ge, ms)) = groups.last_mut()
            && s <= *ge + 1
        {
            *ge = (*ge).max(e);
            ms.push(m.clone());
            continue;
        }
        groups.push((s, e, vec![m.clone()]));
    }

    let orig_lines: Vec<&str> = original.split('\n').collect();
    let mut result_lines: Vec<String> = Vec::new();
    let mut line_idx = 0usize;
    for (s, e, ms) in &groups {
        for line in &orig_lines[line_idx..*s] {
            result_lines.push((*line).to_string());
        }
        let block_start = spans[*s].0;
        let block_end = spans[*e].1;
        let mut block = base[block_start..block_end].to_string();
        apply_replacements(&mut block, ms.clone(), block_start);
        for line in block.split('\n') {
            result_lines.push(line.to_string());
        }
        line_idx = *e + 1;
    }
    for line in &orig_lines[line_idx..] {
        result_lines.push((*line).to_string());
    }
    result_lines.join("\n")
}

fn err_empty(path: &str, i: usize, total: usize) -> String {
    if total == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{i}].oldText must not be empty in {path}.")
    }
}

fn err_not_found(path: &str, i: usize, total: usize) -> String {
    if total == 1 {
        format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        )
    } else {
        format!(
            "Could not find edits[{i}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    }
}

fn err_not_unique(path: &str, i: usize, total: usize, occurrences: usize) -> String {
    if total == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{i}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    }
}

fn err_no_change(path: &str, total: usize) -> String {
    if total == 1 {
        format!("No changes made to {path}. The replacement produced identical content.")
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

/// Minimal LCS line diff. Returns (diff_text, first_changed_line_in_new).
/// Lines are rendered `-<old>   | text`, `+<new>   | text`, `  <new> | text`.
fn render_line_diff(old: &str, new: &str) -> (String, Option<usize>) {
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    let n = a.len();
    let m = b.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = String::new();
    let mut first: Option<usize> = None;
    let mut i = 0usize;
    let mut j = 0usize;
    let mut new_line = 1usize;
    while i < n && j < m {
        if a[i] == b[j] {
            out.push_str(&format!("  {new_line} | {}\n", a[i]));
            new_line += 1;
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            if first.is_none() {
                first = Some(new_line);
            }
            out.push_str(&format!("-{}   | {}\n", i + 1, a[i]));
            i += 1;
        } else {
            if first.is_none() {
                first = Some(new_line);
            }
            out.push_str(&format!("+{new_line}   | {}\n", b[j]));
            new_line += 1;
            j += 1;
        }
    }
    while i < n {
        if first.is_none() {
            first = Some(new_line);
        }
        out.push_str(&format!("-{}   | {}\n", i + 1, a[i]));
        i += 1;
    }
    while j < m {
        if first.is_none() {
            first = Some(new_line);
        }
        out.push_str(&format!("+{new_line}   | {}\n", b[j]));
        new_line += 1;
        j += 1;
    }
    (out, first)
}

/// Applies `edits` to `content`. `content` may have CRLF or a BOM; both are
/// handled. On failure returns a human/LLM-facing error message.
pub fn apply_edits(content: &str, edits: &[Edit], path: &str) -> Result<AppliedEdit, String> {
    if edits.is_empty() {
        return Err(
            "edit tool input is invalid. edits must contain at least one replacement.".to_string(),
        );
    }

    let (has_bom, rest) = split_bom(content);
    let ending = detect_line_ending(rest);
    let normalized = normalize_to_lf(rest);

    // Normalize old/new to LF too (models may emit CRLF inside text).
    let edits: Vec<Edit> = edits
        .iter()
        .map(|e| Edit::new(normalize_to_lf(&e.old_text), normalize_to_lf(&e.new_text)))
        .collect();

    let total = edits.len();
    for (i, e) in edits.iter().enumerate() {
        if e.old_text.is_empty() {
            return Err(err_empty(path, i, total));
        }
    }

    // Pass 1: does any edit require the fuzzy fallback? (exact miss, fuzzy hit)
    let fuzzy_content = trim_end_lines(&normalized);
    let used_fuzzy = edits.iter().any(|e| {
        if normalized.contains(&e.old_text) {
            false
        } else {
            fuzzy_content.contains(&trim_end_lines(&e.old_text))
        }
    });
    let base: String = if used_fuzzy {
        fuzzy_content
    } else {
        normalized.clone()
    };
    let base_fuzzy = trim_end_lines(&base);

    // Pass 2: match all edits against the same base.
    let mut matches: Vec<Match> = Vec::new();
    for (i, e) in edits.iter().enumerate() {
        let found = if let Some(idx) = base.find(&e.old_text) {
            Some((idx, e.old_text.len()))
        } else {
            let fo = trim_end_lines(&e.old_text);
            base_fuzzy.find(&fo).map(|idx| (idx, fo.len()))
        };
        let (index, len) = match found {
            Some(f) => f,
            None => return Err(err_not_found(path, i, total)),
        };
        // Uniqueness is judged in the trim-normalized space (like pi).
        let occurrences = base_fuzzy.split(&trim_end_lines(&e.old_text)).count() - 1;
        if occurrences > 1 {
            return Err(err_not_unique(path, i, total, occurrences));
        }
        matches.push(Match {
            edit_index: i,
            match_index: index,
            match_len: len,
            new_text: e.new_text.clone(),
        });
    }

    matches.sort_by_key(|m| m.match_index);
    for w in matches.windows(2) {
        if w[0].match_index + w[0].match_len > w[1].match_index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                w[0].edit_index, w[1].edit_index
            ));
        }
    }

    let new_normalized = if used_fuzzy {
        apply_preserving(&normalized, &base, &matches)
    } else {
        apply_direct(&normalized, &matches)
    };

    if new_normalized == normalized {
        return Err(err_no_change(path, total));
    }

    let (diff, first_changed_line) = render_line_diff(&normalized, &new_normalized);

    let mut new_content = restore_line_endings(&new_normalized, ending);
    if has_bom {
        new_content.insert(0, '\u{feff}');
    }

    Ok(AppliedEdit {
        new_content,
        replaced_blocks: matches.len(),
        diff,
        first_changed_line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(old: &str, new: &str) -> Edit {
        Edit::new(old, new)
    }

    fn ok(content: &str, edits: &[Edit]) -> AppliedEdit {
        apply_edits(content, edits, "demo.txt").expect("expected success")
    }

    fn err(content: &str, edits: &[Edit]) -> String {
        apply_edits(content, edits, "demo.txt").expect_err("expected error")
    }

    const FILE: &str =
        "fn main() {\n    println!(\"hello\");\n    let x = 1;\n    println!(\"hello\");\n}\n";

    #[test]
    fn unique_exact_match_applies_and_reports_first_changed_line() {
        let applied = ok(FILE, &[e("let x = 1;", "let y = 2;")]);
        assert!(applied.new_content.contains("let y = 2;"));
        assert_eq!(applied.first_changed_line, Some(3));
    }

    #[test]
    fn no_match_errors() {
        let msg = err(FILE, &[e("fn missing()", "fn added()")]);
        assert!(msg.contains("Could not find the exact text"), "msg: {msg}");
    }

    #[test]
    fn multiple_matches_errors_with_count() {
        let msg = err(FILE, &[e("println!(\"hello\");", "dbg!(1);")]);
        assert!(msg.contains("Found 2 occurrences"), "msg: {msg}");
        assert!(msg.contains("must be unique"), "msg: {msg}");
    }

    #[test]
    fn empty_old_text_errors() {
        let msg = err(FILE, &[e("", "x")]);
        assert!(msg.contains("oldText must not be empty"), "msg: {msg}");
    }

    #[test]
    fn overlapping_edits_error() {
        let msg = err(FILE, &[e("let x = 1;", "let x = 9;"), e("x = 1", "x = 8")]);
        assert!(msg.contains("overlap"), "msg: {msg}");
    }

    #[test]
    fn no_op_errors() {
        let msg = err(FILE, &[e("let x = 1;", "let x = 1;")]);
        assert!(msg.contains("No changes made"), "msg: {msg}");
    }

    #[test]
    fn multiple_disjoint_edits_all_match_original() {
        let applied = ok(
            FILE,
            &[
                e(
                    "println!(\"hello\");\n    let x = 1;",
                    "println!(\"hi\");\n    let x = 1;",
                ),
                e("println!(\"hello\");\n}", "println!(\"bye\");\n}"),
            ],
        );
        assert!(applied.new_content.contains("println!(\"hi\");"));
        assert!(applied.new_content.contains("println!(\"bye\");"));
        // both hello call sites changed, one per edit (matched against ORIGINAL)
        assert!(!applied.new_content.contains("println!(\"hello\");"));
        assert_eq!(applied.first_changed_line, Some(2));
    }

    #[test]
    fn crlf_line_endings_preserved() {
        let content = "line1\r\nlet x = 1;\r\nline3\r\n";
        let applied = ok(content, &[e("let x = 1;", "let y = 2;")]);
        assert!(
            applied
                .new_content
                .contains("line1\r\nlet y = 2;\r\nline3\r\n")
        );
        // no stray bare LF outside replacements
        assert!(!applied.new_content.contains("line1\n"));
    }

    #[test]
    fn bom_stripped_and_restored() {
        let content = "\u{feff}fn main() {\n    let x = 1;\n}\n";
        let applied = ok(content, &[e("let x = 1;", "let y = 2;")]);
        assert!(applied.new_content.starts_with('\u{feff}'));
        assert!(applied.new_content.contains("let y = 2;"));
    }

    // --- fuzzy option C: per-line trim_end only ---

    #[test]
    fn fuzzy_matches_trailing_whitespace_in_old_text() {
        // model emits trailing spaces that aren't in the file
        let applied = ok(FILE, &[e("let x = 1;   ", "let y = 9;")]);
        assert!(applied.new_content.contains("let y = 9;"));
        assert_eq!(applied.first_changed_line, Some(3));
    }

    #[test]
    fn fuzzy_preserves_untouched_lines_original_bytes() {
        // oldText has trailing spaces absent from the file → exact fails, fuzzy kicks in.
        // Untouched line 1 keeps its original trailing whitespace.
        let content = "line1   \nlet x = 1;\nline3\n";
        let applied = ok(content, &[e("let x = 1;   ", "let y = 2;")]);
        assert!(
            applied.new_content.starts_with("line1   \n"),
            "got: {:?}",
            applied.new_content
        );
        assert!(
            applied.new_content.contains("let y = 2;"),
            "got: {:?}",
            applied.new_content
        );
        assert!(
            applied.new_content.ends_with("line3\n"),
            "got: {:?}",
            applied.new_content
        );
    }

    #[test]
    fn fuzzy_unique_count_uses_normalized_space() {
        // two lines identical after trim → the same match target → not unique
        let content = "a   \nb   \na   \nlet x = 1;\n";
        let msg = err(content, &[e("a", "z")]);
        assert!(msg.contains("Found 2 occurrences"), "msg: {msg}");
    }

    #[test]
    fn smart_quotes_are_not_folded_by_lightweight_fuzzy() {
        // option C: no Unicode quote folding → must NOT fuzzy-match smart quotes
        let content = "println!(\"hello\");\n";
        let msg = err(
            content,
            &[e("println!(\u{201c}hello\u{201d});", "println!(\"HI\");")],
        );
        assert!(msg.contains("Could not find"), "msg: {msg}");
    }

    #[test]
    fn diff_shows_removed_and_added_lines() {
        let applied = ok(FILE, &[e("let x = 1;", "let y = 2;")]);
        assert!(
            applied.diff.contains("-3   |     let x = 1;"),
            "diff:\n{}",
            applied.diff
        );
        assert!(
            applied.diff.contains("+3   |     let y = 2;"),
            "diff:\n{}",
            applied.diff
        );
    }

    #[test]
    fn reports_replaced_block_count() {
        let content = "a\nb\nc\n";
        let applied = ok(content, &[e("a", "x"), e("c", "y")]);
        assert_eq!(applied.replaced_blocks, 2);
        let single = ok(content, &[e("a", "x")]);
        assert_eq!(single.replaced_blocks, 1);
    }

    #[test]
    fn fuzzy_match_ending_in_newline_preserves_later_trailing_whitespace() {
        // fuzzy is active (first oldText has trailing space) AND a second edit's
        // oldText ends with '\n'; the later untouched line keeps its trailing
        // spaces instead of being rewritten from the trimmed base.
        let content = "let a = 1;   \nlet b = 2;\nkeep   \n";
        let applied = ok(
            content,
            &[
                e("let a = 1;   ", "let a = 1;"),
                e("let b = 2;\n", "let b = 2;\n"),
            ],
        );
        assert_eq!(applied.new_content, "let a = 1;\nlet b = 2;\nkeep   \n");
    }

    #[test]
    fn pure_deletion_reports_first_changed_line() {
        let content = "a\nb\nc\n";
        let applied = ok(content, &[e("b\n", "")]);
        assert_eq!(applied.new_content, "a\nc\n");
        assert_eq!(applied.first_changed_line, Some(2));
    }
}
