//! Pure per-tool compact call-title composer (ticket 06, ADR-0006 D2): maps a
//! built-in tool's raw JSON arguments to pi's `format*Call` title shapes —
//! `read <path>:<range>`, `ls <path>`, `grep /pattern/ in <path>`, `find
//! <pattern> in <path>`, `edit`/`write <path>`, `$ bash command` — as styled
//! [`CallPart`] runs. Unknown tools, unparseable JSON, and missing required
//! fields return `None` so the caller keeps pi's fallback (bold name + pretty
//! JSON args).
//!
//! Mirrors pi's `formatReadCall`/`formatReadLineRange` (`read.ts`),
//! `formatLsCall` (`ls.ts`), `formatGrepCall` (`grep.ts`), `formatFindCall`
//! (`find.ts`), `formatEditCall` (`edit.ts`), `formatWriteCall` (`write.ts`)
//! and `formatShellCall` (`bash.ts`): names/commands in bold `toolTitle`,
//! paths/patterns in `accent`, read line ranges in `warning`, secondary
//! suffixes (` in …`, ` (limit N)`) in `toolOutput`, empty-path default `.`,
//! empty command fallback `...`. Everything here is pure and unit-tested.

use serde_json::Value;

use crate::theme::Token;

/// One styled run of a tool-call title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallPart {
    /// The text of this run (spaces included).
    pub text: String,
    /// Foreground semantic token for the run.
    pub fg: Token,
    /// Whether the run renders bold (pi names/commands are bold).
    pub bold: bool,
}

/// Compose a built-in tool call's compact title from its raw JSON arguments.
///
/// Returns `None` (caller falls back to bold name + pretty JSON args) when the
/// args are unparseable JSON, the tool name is unknown, a required field is
/// missing or not a string, or a read range argument is present but not a
/// non-negative integer.
pub fn tool_call_title(name: &str, args_raw: &str) -> Option<Vec<CallPart>> {
    let value: Value = serde_json::from_str(args_raw.trim()).ok()?;
    let obj = value.as_object()?;
    match name {
        "read" => read_call(obj),
        "ls" => ls_call(obj),
        "grep" => grep_call(obj),
        "find" => find_call(obj),
        "edit" => edit_call(obj),
        "write" => write_call(obj),
        "bash" => bash_call(obj),
        _ => None,
    }
}

/// The joined plain text of a title (test helper: asserts the exact shape a
/// title renders as).
pub fn parts_text(parts: &[CallPart]) -> String {
    parts.iter().map(|p| p.text.as_str()).collect()
}

fn name_part(name: &str) -> CallPart {
    CallPart {
        text: name.to_string(),
        fg: Token::ToolTitle,
        bold: true,
    }
}

/// The ` ` separator between the bold tool name and its arguments carries the
/// name's style (pi's `" "` is inside the same `theme.fg("toolTitle", bold)`).
fn gap() -> CallPart {
    CallPart {
        text: " ".to_string(),
        fg: Token::ToolTitle,
        bold: true,
    }
}

/// A string field: `None` when absent or not a JSON string (callers decide
/// whether the field is required or optional).
fn str_field<'a>(obj: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

/// A required path: absent/non-string → `None`; empty string renders as pi's
/// gray `...` fallback.
fn req_path(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    match str_field(obj, "path") {
        None => None,
        Some("") => Some(vec![CallPart {
            text: "...".to_string(),
            fg: Token::ToolOutput,
            bold: false,
        }]),
        Some(path) => Some(path_part(path)),
    }
}

/// An optional scope path (`ls`/`grep`/`find`): absent or empty → accent `.`.
fn opt_path_parts(obj: &serde_json::Map<String, Value>) -> Vec<CallPart> {
    match str_field(obj, "path") {
        Some("") | None => path_part("."),
        Some(path) => path_part(path),
    }
}

fn path_part(path: &str) -> Vec<CallPart> {
    vec![CallPart {
        text: path.to_string(),
        fg: Token::Accent,
        bold: false,
    }]
}

/// The `:<start[-end]>` read line range (pi `formatReadLineRange`): rendered
/// only when `offset`/`limit` are present; both are 1-indexed, and the range
/// end is `start + limit - 1`. A range arg present but not a non-negative
/// integer makes the whole call fall back (returns `None`).
fn read_range(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    let offset = obj.get("offset").and_then(Value::as_u64);
    let limit = obj.get("limit").and_then(Value::as_u64);
    // A range argument that exists but is not a non-negative integer is
    // unusable: the caller falls back to the pretty-JSON title.
    if (obj.contains_key("offset") && offset.is_none())
        || (obj.contains_key("limit") && limit.is_none())
    {
        return None;
    }
    if offset.is_none() && limit.is_none() {
        return Some(Vec::new());
    }
    // `offset` is 1-indexed; clamp below 1 to the first line so the rendered
    // range always agrees with the engine's `slice_lines` clamping.
    let start = offset.map(|o| o.max(1)).unwrap_or(1);
    let end = if limit.is_some() {
        Some(start.saturating_add(limit.unwrap_or(0)).saturating_sub(1))
    } else {
        None
    };
    let range = match end {
        Some(end) if end > 0 => format!(":{start}-{end}"),
        // limit 0 yields an empty window: pi's number 0 is falsy, so only the
        // start renders.
        _ => format!(":{start}"),
    };
    Some(vec![CallPart {
        text: range,
        fg: Token::Warning,
        bold: false,
    }])
}

fn read_call(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    let mut parts = Vec::new();
    parts.push(name_part("read"));
    parts.push(gap());
    parts.extend(req_path(obj)?);
    parts.extend(read_range(obj)?);
    Some(parts)
}

fn ls_call(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    let mut parts = vec![name_part("ls"), gap()];
    parts.extend(opt_path_parts(obj));
    if let Some(limit) = obj.get("limit").and_then(Value::as_u64) {
        parts.push(CallPart {
            text: format!(" (limit {limit})"),
            fg: Token::ToolOutput,
            bold: false,
        });
    }
    Some(parts)
}

fn grep_call(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    let pattern = str_field(obj, "pattern")?;
    let mut parts = vec![name_part("grep"), gap()];
    parts.push(CallPart {
        text: format!("/{pattern}/"),
        fg: Token::Accent,
        bold: false,
    });
    let scope = opt_path_parts(obj);
    parts.push(CallPart {
        text: " in ".to_string(),
        fg: Token::ToolOutput,
        bold: false,
    });
    parts.extend(scope);
    Some(parts)
}

fn find_call(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    let mut parts = vec![name_part("find"), gap()];
    // `pattern` is optional (slimcode filters by filename substring); empty or
    // absent renders `.` like the scope default.
    match str_field(obj, "pattern") {
        Some("") | None => parts.extend(path_part(".")),
        Some(pattern) => parts.extend(path_part(pattern)),
    }
    parts.push(CallPart {
        text: " in ".to_string(),
        fg: Token::ToolOutput,
        bold: false,
    });
    parts.extend(opt_path_parts(obj));
    Some(parts)
}

fn edit_call(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    let mut parts = vec![name_part("edit"), gap()];
    parts.extend(req_path(obj)?);
    Some(parts)
}

fn write_call(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    let mut parts = vec![name_part("write"), gap()];
    parts.extend(req_path(obj)?);
    Some(parts)
}

fn bash_call(obj: &serde_json::Map<String, Value>) -> Option<Vec<CallPart>> {
    let mut parts = vec![CallPart {
        text: "$ ".to_string(),
        fg: Token::ToolTitle,
        bold: true,
    }];
    match str_field(obj, "command") {
        Some("") | None => parts.push(CallPart {
            text: "...".to_string(),
            fg: Token::ToolOutput,
            bold: false,
        }),
        Some(command) => parts.push(CallPart {
            text: command.to_string(),
            fg: Token::ToolTitle,
            bold: true,
        }),
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: serde_json::Value) -> Option<Vec<CallPart>> {
        tool_call_title(name, &args.to_string())
    }

    fn token_of(parts: &[CallPart], needle: &str) -> Token {
        parts
            .iter()
            .find(|p| p.text.contains(needle))
            .map(|p| p.fg)
            .unwrap_or_else(|| panic!("no part contains {needle:?}: {parts:?}"))
    }

    // --- read --------------------------------------------------------------

    #[test]
    fn read_title_names_path_in_accent() {
        let parts = call("read", serde_json::json!({"path": "a.txt"})).unwrap();
        assert_eq!(parts_text(&parts), "read a.txt");
        assert_eq!(parts[0].fg, Token::ToolTitle);
        assert!(parts[0].bold);
        assert_eq!(token_of(&parts, "a.txt"), Token::Accent);
        // No range args → no `:` suffix.
        assert!(!parts.iter().any(|p| p.text.contains(':')));
    }

    #[test]
    fn read_title_shows_offset_only_range_in_warning() {
        let parts = call("read", serde_json::json!({"path": "a.txt", "offset": 5})).unwrap();
        assert_eq!(parts_text(&parts), "read a.txt:5");
        assert_eq!(token_of(&parts, ":5"), Token::Warning);
    }

    #[test]
    fn read_title_shows_offset_limit_range_in_warning() {
        // 1-indexed window: offset 5, limit 10 covers lines 5..=14.
        let parts = call(
            "read",
            serde_json::json!({"path": "a.txt", "offset": 5, "limit": 10}),
        )
        .unwrap();
        assert_eq!(parts_text(&parts), "read a.txt:5-14");
        assert_eq!(token_of(&parts, ":5-14"), Token::Warning);
    }

    #[test]
    fn read_title_limit_without_offset_starts_at_line_one() {
        let parts = call("read", serde_json::json!({"path": "a.txt", "limit": 3})).unwrap();
        assert_eq!(parts_text(&parts), "read a.txt:1-3");
    }

    #[test]
    fn read_title_clamps_zero_offset_to_line_one() {
        // `offset` is 1-indexed; the engine clamps below-1 offsets to the
        // first line, so the title must not claim a `:0-…` window.
        let parts = call(
            "read",
            serde_json::json!({"path": "a.txt", "offset": 0, "limit": 3}),
        )
        .unwrap();
        assert_eq!(parts_text(&parts), "read a.txt:1-3");
        let parts = call("read", serde_json::json!({"path": "a.txt", "offset": 0})).unwrap();
        assert_eq!(parts_text(&parts), "read a.txt:1");
    }

    #[test]
    fn read_missing_or_invalid_fields_fall_back() {
        // Missing required path → fallback.
        assert!(call("read", serde_json::json!({"offset": 2})).is_none());
        assert!(call("read", serde_json::json!({})).is_none());
        // Non-string path or range → fallback.
        assert!(call("read", serde_json::json!({"path": 7})).is_none());
        assert!(call("read", serde_json::json!({"path": "a", "offset": "x"})).is_none());
        assert!(call("read", serde_json::json!({"path": "a", "offset": -1})).is_none());
        assert!(call("read", serde_json::json!({"path": "a", "offset": 1.5})).is_none());
        // Unparseable JSON → fallback.
        assert!(tool_call_title("read", "a.txt").is_none());
        assert!(tool_call_title("read", "").is_none());
    }

    #[test]
    fn read_empty_path_uses_gray_ellipsis() {
        let parts = call("read", serde_json::json!({"path": ""})).unwrap();
        assert_eq!(parts_text(&parts), "read ...");
        assert_eq!(token_of(&parts, "..."), Token::ToolOutput);
    }

    // --- ls ----------------------------------------------------------------

    #[test]
    fn ls_title_defaults_empty_path_to_dot() {
        let parts = call("ls", serde_json::json!({})).unwrap();
        assert_eq!(parts_text(&parts), "ls .");
        assert_eq!(token_of(&parts, "."), Token::Accent);
        let parts = call("ls", serde_json::json!({"path": ""})).unwrap();
        assert_eq!(parts_text(&parts), "ls .");
    }

    #[test]
    fn ls_title_shows_path_and_optional_limit_suffix() {
        let parts = call("ls", serde_json::json!({"path": "src"})).unwrap();
        assert_eq!(parts_text(&parts), "ls src");
        let parts = call("ls", serde_json::json!({"path": "src", "limit": 25})).unwrap();
        assert_eq!(parts_text(&parts), "ls src (limit 25)");
        // The suffix is a muted toolOutput run, not accent.
        assert_eq!(token_of(&parts, "(limit 25)"), Token::ToolOutput);
    }

    // --- grep / find -------------------------------------------------------

    #[test]
    fn grep_title_wraps_pattern_in_slashes_with_scope() {
        let parts = call("grep", serde_json::json!({"pattern": "todo"})).unwrap();
        assert_eq!(parts_text(&parts), "grep /todo/ in .");
        assert_eq!(token_of(&parts, "/todo/"), Token::Accent);
        assert_eq!(token_of(&parts, " in "), Token::ToolOutput);
        let parts = call(
            "grep",
            serde_json::json!({"pattern": "todo", "path": "src"}),
        )
        .unwrap();
        assert_eq!(parts_text(&parts), "grep /todo/ in src");
    }

    #[test]
    fn grep_missing_pattern_falls_back() {
        assert!(call("grep", serde_json::json!({})).is_none());
        assert!(call("grep", serde_json::json!({"path": "src"})).is_none());
    }

    #[test]
    fn find_title_shows_pattern_and_scope_with_dot_defaults() {
        let parts = call(
            "find",
            serde_json::json!({"pattern": "main.rs", "path": "src"}),
        )
        .unwrap();
        assert_eq!(parts_text(&parts), "find main.rs in src");
        // Both pattern and scope default to `.`.
        let parts = call("find", serde_json::json!({})).unwrap();
        assert_eq!(parts_text(&parts), "find . in .");
    }

    // --- edit / write ------------------------------------------------------

    #[test]
    fn edit_and_write_titles_name_the_path_in_accent() {
        let parts = call("edit", serde_json::json!({"path": "a.txt"})).unwrap();
        assert_eq!(parts_text(&parts), "edit a.txt");
        assert_eq!(token_of(&parts, "a.txt"), Token::Accent);
        let parts = call(
            "write",
            serde_json::json!({"path": "b.txt", "content": "x"}),
        )
        .unwrap();
        assert_eq!(parts_text(&parts), "write b.txt");
        // Missing path → fallback.
        assert!(call("edit", serde_json::json!({})).is_none());
        assert!(call("write", serde_json::json!({"content": "x"})).is_none());
    }

    // --- bash --------------------------------------------------------------

    #[test]
    fn bash_title_is_bold_dollar_command() {
        let parts = call("bash", serde_json::json!({"command": "ls -la"})).unwrap();
        assert_eq!(parts_text(&parts), "$ ls -la");
        assert!(parts.iter().all(|p| p.fg == Token::ToolTitle && p.bold));
    }

    #[test]
    fn bash_empty_command_uses_gray_ellipsis() {
        let parts = call("bash", serde_json::json!({})).unwrap();
        assert_eq!(parts_text(&parts), "$ ...");
        assert_eq!(token_of(&parts, "..."), Token::ToolOutput);
        let parts = call("bash", serde_json::json!({"command": ""})).unwrap();
        assert_eq!(parts_text(&parts), "$ ...");
    }

    // --- fallbacks ---------------------------------------------------------

    #[test]
    fn unknown_tool_and_non_object_args_fall_back() {
        assert!(call("teleport", serde_json::json!({"path": "x"})).is_none());
        assert!(tool_call_title("read", "[]").is_none());
        assert!(tool_call_title("read", "42").is_none());
        assert!(tool_call_title("read", "\"str\"").is_none());
    }
}
