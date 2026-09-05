//! PROTOTYPE — slimcode ticket 03: edit tool semantics.
//! Throwaway. Answers: which match/apply semantics should crates/agent's `edit` tool lock for v1?
//!
//! Two candidate engines:
//!   - `apply_exact`: exact old_text match only. Deterministic, no side effects.
//!   - `apply_fuzzy_simple`: exact first, then whitespace/Unicode-tolerant fuzzy on a normalized
//!     copy of the whole file (side effect: unchanged lines get re-normalized too — pi preserves
//!     original bytes via its `applyReplacementsPreservingUnchangedLines`; that machinery is the
//!     extra cost of full fuzzy).
//!
//! Run:  cargo run            (demo battery)
//!       cargo run -- <file> <old> <new>   (apply one edit to a real file, print result)
//! No dependencies, pure std.

use std::env;
use std::fs;
use std::process;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Edit {
    old_text: String,
    new_text: String,
}

struct Applied {
    content: String,
    diff: String,
    first_changed_line: Option<usize>,
    fuzzy_used: bool,
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

fn normalize_lf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn detect_line_ending(s: &str) -> &'static str {
    let crlf = s.find("\r\n");
    let lf = s.find('\n');
    match (crlf, lf) {
        (Some(c), Some(l)) if c < l => "\r\n",
        _ => "\n",
    }
}

fn restore_line_endings(s: &str, ending: &'static str) -> String {
    if ending == "\r\n" {
        s.replace('\n', "\r\n")
    } else {
        s.to_string()
    }
}

fn strip_bom(s: &str) -> (bool, &str) {
    (s.starts_with('\u{feff}'), s.strip_prefix('\u{feff}').unwrap_or(s))
}

/// pi's normalizeForFuzzyMatch: per-line trim_end + smart-quote/dash/space ASCII folding.
/// (No NFKC in std; we fold the exact codepoint ranges pi handles.)
fn normalize_for_fuzzy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let folded = match ch {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            _ => ch,
        };
        out.push(folded);
    }
    // strip trailing whitespace per line
    out.split('\n')
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Find old_text in content: exact first, then fuzzy on a normalized copy.
/// Returns (found, byte_index, match_len, used_fuzzy, content_for_replacement).
fn fuzzy_find(content: &str, old_text: &str) -> (bool, usize, usize, bool, String) {
    if let Some(i) = content.find(old_text) {
        return (true, i, old_text.len(), false, content.to_string());
    }
    let fuzzy_content = normalize_for_fuzzy(content);
    let fuzzy_old = normalize_for_fuzzy(old_text);
    if let Some(i) = fuzzy_content.find(&fuzzy_old) {
        (true, i, fuzzy_old.len(), true, fuzzy_content)
    } else {
        (false, 0, 0, false, content.to_string())
    }
}

fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fc = normalize_for_fuzzy(content);
    let fo = normalize_for_fuzzy(old_text);
    fc.split(&fo).count().saturating_sub(1)
}

// ---------------------------------------------------------------------------
// Apply engines
// ---------------------------------------------------------------------------

struct Match {
    edit_index: usize,
    match_index: usize,
    match_len: usize,
    new_text: String,
}

fn err_not_found(path: &str, n: usize, total: usize) -> String {
    if total == 1 {
        format!("Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines.")
    } else {
        format!("Could not find edits[{n}] in {path}. The oldText must match exactly including all whitespace and newlines.")
    }
}

fn err_duplicate(path: &str, n: usize, total: usize, occurrences: usize) -> String {
    if total == 1 {
        format!("Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique.")
    } else {
        format!("Found {occurrences} occurrences of edits[{n}] in {path}. Each oldText must be unique. Please provide more context to make it unique.")
    }
}

fn err_empty(path: &str, n: usize, total: usize) -> String {
    if total == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{n}].oldText must not be empty in {path}.")
    }
}

fn err_no_change(path: &str, total: usize) -> String {
    if total == 1 {
        format!("No changes made to {path}. The replacement produced identical content.")
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

/// Core matching + ordering + overlap checks. Returns matched edits in ascending offset order.
/// `allow_fuzzy` selects exact-only vs exact-then-fuzzy.
fn find_matches(
    content: &str,
    edits: &[Edit],
    path: &str,
    allow_fuzzy: bool,
) -> Result<Vec<Match>, String> {
    let total = edits.len();
    let mut matched: Vec<Match> = Vec::new();

    for (i, edit) in edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(err_empty(path, i, total));
        }
        let (found, idx, len, _used_fuzzy, _) = if allow_fuzzy {
            fuzzy_find(content, &edit.old_text)
        } else {
            // exact only: no normalization at all
            match content.find(&edit.old_text) {
                Some(i) => (true, i, edit.old_text.len(), false, String::new()),
                None => (false, 0, 0, false, String::new()),
            }
        };
        if !found {
            return Err(err_not_found(path, i, total));
        }
        let occurrences = count_occurrences(content, &edit.old_text);
        if occurrences > 1 {
            return Err(err_duplicate(path, i, total, occurrences));
        }
        matched.push(Match { edit_index: i, match_index: idx, match_len: len, new_text: edit.new_text.clone() });
    }

    matched.sort_by_key(|m| m.match_index);
    for w in matched.windows(2) {
        let a = &w[0];
        let b = &w[1];
        if a.match_index + a.match_len > b.match_index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                a.edit_index, b.edit_index
            ));
        }
    }
    Ok(matched)
}

/// Exact-only engine: match and replace on the original (LF-normalized) content.
/// No fuzzy fallback, no normalization side effects.
fn apply_exact(normalized: &str, edits: &[Edit], path: &str) -> Result<Applied, String> {
    let mut matched = find_matches(normalized, edits, path, false)?;
    let old_content = normalized.to_string();
    // apply in reverse so offsets stay valid
    matched.sort_by_key(|m| std::cmp::Reverse(m.match_index));
    let mut new_content = normalized.to_string();
    for m in &matched {
        new_content.replace_range(m.match_index..m.match_index + m.match_len, &m.new_text);
    }
    if old_content == new_content {
        return Err(err_no_change(path, edits.len()));
    }
    let (diff, first) = render_diff(&old_content, &new_content);
    Ok(Applied { content: new_content, diff, first_changed_line: first, fuzzy_used: false })
}

/// Fuzzy-fallback engine (simple): exact first; if any edit needed fuzzy, the WHOLE file is
/// rewritten from the normalized copy. Side effect: trailing whitespace on untouched lines is
/// stripped and unicode punctuation is ASCII-folded everywhere. pi avoids this via
/// `applyReplacementsPreservingUnchangedLines` (extra machinery).
fn apply_fuzzy_simple(normalized: &str, edits: &[Edit], path: &str) -> Result<Applied, String> {
    let mut matched = find_matches(normalized, edits, path, true)?;
    let used_fuzzy = {
        // detect whether any edit required fuzzy by re-checking exact on the original
        edits.iter().any(|e| normalized.find(&e.old_text).is_none())
    };
    let old_content = normalized.to_string();
    let base = if used_fuzzy { normalize_for_fuzzy(normalized) } else { normalized.to_string() };
    let mut new_content = base.clone();
    matched.sort_by_key(|m| std::cmp::Reverse(m.match_index));
    for m in &matched {
        new_content.replace_range(m.match_index..m.match_index + m.match_len, &m.new_text);
    }
    if old_content == new_content {
        return Err(err_no_change(path, edits.len()));
    }
    let (diff, first) = render_diff(&old_content, &new_content);
    Ok(Applied { content: new_content, diff, first_changed_line: first, fuzzy_used: used_fuzzy })
}

// ---------------------------------------------------------------------------
// Minimal line diff (LCS) + first changed line
// ---------------------------------------------------------------------------

fn render_diff(old: &str, new: &str) -> (String, Option<usize>) {
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    // LCS lengths
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
    let mut i = 0;
    let mut j = 0;
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    while i < n && j < m {
        if a[i] == b[j] {
            out.push_str(&format!("  {} | {}\n", new_line, a[i]));
            old_line += 1;
            new_line += 1;
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push_str(&format!("-{}   | {}\n", old_line, a[i]));
            old_line += 1;
            i += 1;
        } else {
            if first.is_none() {
                first = Some(new_line);
            }
            out.push_str(&format!("+{}   | {}\n", new_line, b[j]));
            new_line += 1;
            j += 1;
        }
    }
    while i < n {
        out.push_str(&format!("-{}   | {}\n", old_line, a[i]));
        old_line += 1;
        i += 1;
    }
    while j < m {
        if first.is_none() {
            first = Some(new_line);
        }
        out.push_str(&format!("+{}   | {}\n", new_line, b[j]));
        new_line += 1;
        j += 1;
    }
    (out, first)
}

// ---------------------------------------------------------------------------
// Demo battery
// ---------------------------------------------------------------------------

fn run_case(name: &str, content: &str, edits: &[Edit], engine: fn(&str, &[Edit], &str) -> Result<Applied, String>) {
    let normalized = normalize_lf(content);
    println!("\n===== {name} =====");
    for (i, e) in edits.iter().enumerate() {
        println!("  edit[{i}]: old={:?} new={:?}", e.old_text, e.new_text);
    }
    println!("  file (LF): {:?}", normalized);
    match engine(&normalized, edits, "demo.txt") {
        Ok(applied) => {
            println!("  -> OK (fuzzy={})", applied.fuzzy_used);
            println!("  -> first changed line: {:?}", applied.first_changed_line);
            println!("  -> diff:\n{}", indent(&applied.diff));
            println!("  -> new content: {:?}", applied.content);
        }
        Err(e) => println!("  -> ERROR: {e}"),
    }
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("     {l}")).collect::<Vec<_>>().join("\n")
}

fn demo() {
    let file = "fn main() {\n    println!(\"hello\");\n    let x = 1;\n    println!(\"hello\");\n}\n";

    // --- shared edge battery (behavior is engine-independent for these) ---
    println!("══════════════ EXACT ENGINE ══════════════");
    run_case("1. unique exact match", file, &[Edit { old_text: "let x = 1;".into(), new_text: "let y = 2;".into() }], apply_exact);
    run_case("2. no match", file, &[Edit { old_text: "fn missing()".into(), new_text: "fn added()".into() }], apply_exact);
    run_case("3. multiple matches (duplicate)", file, &[Edit { old_text: "println!(\"hello\");".into(), new_text: "dbg!(1);".into() }], apply_exact);
    run_case("4. empty oldText", file, &[Edit { old_text: "".into(), new_text: "x".into() }], apply_exact);
    run_case("5. overlapping edits", file, &[
        Edit { old_text: "let x = 1;".into(), new_text: "let x = 9;".into() },
        Edit { old_text: "x = 1".into(), new_text: "x = 8".into() },
    ], apply_exact);
    run_case("6. no-op (old==new)", file, &[Edit { old_text: "let x = 1;".into(), new_text: "let x = 1;".into() }], apply_exact);
    run_case("7. multi-edit disjoint, all matched vs ORIGINAL", file, &[
        Edit { old_text: "println!(\"hello\");\n    let x = 1;".into(), new_text: "println!(\"hi\");\n    let x = 1;".into() },
        Edit { old_text: "println!(\"hello\");\n}".into(), new_text: "println!(\"bye\");\n}".into() },
    ], apply_exact);

    // --- the fuzzy decision: same slightly-off old_text under each engine ---
    println!("\n══════════════ FUZZY DECISION ══════════════");
    let fuzzy_file = "fn main() {\n    println!(\"hello\");\n    let x = 1;\n}\n";
    let smart_quote_edit = Edit {
        old_text: "println!(\u{201c}hello\u{201d});".into(), // “hello” smart quotes
        new_text: "println!(\"HELLO\");".into(),
    };
    let trailing_ws_edit = Edit {
        old_text: "let x = 1;   ".into(), // trailing spaces model might emit
        new_text: "let y = 9;".into(),
    };

    println!("--- exact engine, oldText has smart quotes → what happens ---");
    run_case("8. exact-only vs smart quotes", fuzzy_file, &[smart_quote_edit.clone()], apply_exact);
    println!("--- fuzzy engine, same input ---");
    run_case("8b. fuzzy vs smart quotes", fuzzy_file, &[smart_quote_edit.clone()], apply_fuzzy_simple);

    println!("--- exact engine, oldText has trailing whitespace ---");
    run_case("9. exact-only vs trailing ws", fuzzy_file, &[trailing_ws_edit.clone()], apply_exact);
    println!("--- fuzzy engine, same input ---");
    run_case("9b. fuzzy vs trailing ws", fuzzy_file, &[trailing_ws_edit.clone()], apply_fuzzy_simple);

    // CRLF preservation
    let crlf = "line1\r\nlet x = 1;\r\nline3\r\n";
    println!("\n══════════════ LINE ENDINGS ══════════════");
    let normalized_crlf = normalize_lf(crlf);
    let ending = detect_line_ending(crlf);
    match apply_exact(&normalized_crlf, &[Edit { old_text: "let x = 1;".into(), new_text: "let y = 2;".into() }], "demo.txt") {
        Ok(a) => {
            let restored = restore_line_endings(&a.content, ending);
            println!("  CRLF file edited; endings preserved: {:?}", restored);
        }
        Err(e) => println!("  ERROR: {e}"),
    }
}

// ---------------------------------------------------------------------------
// CLI mode: edit a real file
// ---------------------------------------------------------------------------

fn cli_mode(args: &[String]) -> i32 {
    if args.len() != 3 {
        eprintln!("usage: edit-semantics-proto <file> <old> <new>   OR   edit-semantics-proto --demo");
        return 2;
    }
    let path = &args[0];
    let old = &args[1];
    let new = &args[2];
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not read {path}: {e}");
            return 1;
        }
    };
    let (has_bom, rest) = strip_bom(&raw);
    let normalized = normalize_lf(rest);
    let ending = detect_line_ending(rest);
    let edit = Edit { old_text: old.clone(), new_text: new.clone() };
    match apply_fuzzy_simple(&normalized, &[edit], path) {
        Ok(applied) => {
            let mut content = applied.content;
            if has_bom {
                content.insert_str(0, "\u{feff}");
            }
            let content = restore_line_endings(&content, ending);
            if let Err(e) = fs::write(path, &content) {
                eprintln!("could not write {path}: {e}");
                return 1;
            }
            println!("applied. diff:\n{}", applied.diff);
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--demo" {
        demo();
        return;
    }
    process::exit(cli_mode(&args));
}
