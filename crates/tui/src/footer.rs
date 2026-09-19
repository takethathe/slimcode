//! Pure, unit-tested footer formatting for the pi-style two-line dock footer
//! (ADR-0006 D5). Ports the formatting from pi's `footer.ts`
//! (`formatTokens`, `formatCwdForFooter`, stats composition) so the numbers
//! and layout match pi without importing any of its layout engine.
//!
//! All functions here are pure: they take plain strings/numbers and return
//! plain strings. The `App` composes them into styled `Line`s at draw time;
//! the terminal shell only feeds in the cwd / session / model / usage.

/// Port of pi `formatTokens(count)`: compact token count formatting.
/// `<1000` plain; `<10000` one-decimal `k`; `<1e6` rounded `k`; `<1e7`
/// one-decimal `M`; else rounded `M`.
pub fn format_tokens(count: u64) -> String {
    if count < 1000 {
        count.to_string()
    } else if count < 10_000 {
        format!("{:.1}k", count as f64 / 1000.0)
    } else if count < 1_000_000 {
        format!("{}k", (count as f64 / 1000.0).round() as u64)
    } else if count < 10_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else {
        format!("{}M", (count as f64 / 1_000_000.0).round() as u64)
    }
}

/// Port of pi `formatCwdForFooter(cwd, home)`: shorten an absolute cwd to
/// `~` / `~/rel` when it lives inside the user's home directory; otherwise
/// return the path unchanged. `home` is `None` when the environment has no
/// home (non-interactive / tests).
pub fn format_cwd_for_footer(cwd: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return cwd.to_string();
    };
    let resolved_cwd = std::path::Path::new(cwd);
    let resolved_home = std::path::Path::new(home);
    // Strip trailing separators so `/home/u/` == `/home/u` for the home match.
    let is_inside_home = resolved_cwd.starts_with(resolved_home);
    if !is_inside_home {
        return cwd.to_string();
    }
    let rel = resolved_cwd
        .strip_prefix(resolved_home)
        .unwrap_or(resolved_cwd);
    if rel.as_os_str().is_empty() {
        "~".to_string()
    } else {
        format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display())
    }
}

/// The session's accumulated token-usage view for the footer stats line.
/// Mirrors pi's `usageTotals` (input, output, cacheRead, cacheWrite) plus the
/// cache hit rate. slimcode's provider accumulates into [`TokenUsage`]
/// already, so the footer reads the same struct the `/usage` line uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FooterUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl FooterUsage {
    /// Plain constructor (ADR-0014 D1): the CLI maps the provider's usage into
    /// these four counters, so this crate never names the AI type.
    pub fn new(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write,
        }
    }
}

/// The cache hit percentage of the prompt tokens (`cached / prompt`), rounded
/// to one decimal and trimmed of a trailing `.0`. `None` when there is no
/// prompt usage or no cache activity (pi omits `CH…` then).
fn cache_hit_percent(u: &FooterUsage) -> Option<String> {
    let prompt = u.input;
    if prompt == 0 || (u.cache_read == 0 && u.cache_write == 0) {
        return None;
    }
    let tenths = u.cache_read.saturating_mul(1000).saturating_add(prompt / 2) / prompt;
    let whole = tenths / 10;
    let frac = tenths % 10;
    Some(if frac == 0 {
        format!("{whole}")
    } else {
        format!("{whole}.{frac}")
    })
}

/// Build the stats part of footer line 2: `↑in ↓out Rcache WcacheWrite
/// CH{pct}%` with zero parts omitted (pi omits each zero part). Returns an
/// empty string when there is no usage at all.
pub fn stats_parts(usage: &FooterUsage) -> Vec<String> {
    let mut parts = Vec::new();
    if usage.input > 0 {
        parts.push(format!("↑{}", format_tokens(usage.input)));
    }
    if usage.output > 0 {
        parts.push(format!("↓{}", format_tokens(usage.output)));
    }
    if usage.cache_read > 0 {
        parts.push(format!("R{}", format_tokens(usage.cache_read)));
    }
    if usage.cache_write > 0 {
        parts.push(format!("W{}", format_tokens(usage.cache_write)));
    }
    if let Some(pct) = cache_hit_percent(usage) {
        parts.push(format!("CH{pct}%"));
    }
    parts
}

/// Compose footer line 2 content: `stats` left-aligned, `model` right-aligned
/// with at least two spaces between them, truncating the right side when the
/// terminal is narrow (pi footer.ts lines 175–220). The `model` is dropped
/// entirely if even a truncated version cannot fit.
pub fn stats_line(stats: &[String], model: &str, width: usize) -> String {
    let stats_left = stats.join(" ");
    let stats_width = display_width(&stats_left);
    if stats_width >= width {
        // The stats alone fill the line; drop the model (pi truncates stats
        // with "..." first, but slimcode's stats are short).
        return truncate_width_str(&stats_left, width);
    }
    let min_padding = 2;
    let total_needed = stats_width + min_padding + display_width(model);
    if total_needed <= width {
        let padding = " ".repeat(width - stats_width - display_width(model));
        return format!("{stats_left}{padding}{model}");
    }
    // Not enough room for the full model: truncate it with no ellipsis (pi).
    let available = width - stats_width - min_padding;
    if available > 0 {
        let truncated = truncate_width_str(model, available);
        let pad = " ".repeat(width - stats_width - display_width(&truncated));
        format!("{stats_left}{pad}{truncated}")
    } else {
        stats_left
    }
}

/// The context-window usage the footer's context segment shows: the
/// estimated token share of the assumed window (`percent`, 0–100) and the
/// window size. Always fully determined — slimcode uses a constant window and
/// estimates it itself, so there is no pi `?` unknown state (ticket 04).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextUsage {
    pub percent: f64,
    pub window: u64,
}

impl ContextUsage {
    /// Plain constructor (ADR-0014 D1): the CLI computes the percentage with
    /// the app layer's estimator and window, so this crate never names an
    /// estimator type.
    pub fn new(percent: f64, window: u64) -> Self {
        Self { percent, window }
    }
}

/// How the footer colors the context segment (pi's thresholds, ticket 04).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextLevel {
    /// <= 70%: dim, nothing to worry about.
    Normal,
    /// > 70%: warn color — the window is filling up.
    Warn,
    /// > 90%: error color — auto-compaction is close.
    Critical,
}

/// Compose the context segment text + level for footer line 2: `45.3%/200k`.
/// The percentage is rounded to one decimal and trimmed of a trailing `.0`
/// (`0%`, `45.3%`); the window uses [`format_tokens`] (`200_000` → `200k`).
pub fn context_part(percent: f64, window: u64) -> (String, ContextLevel) {
    let level = if percent > 90.0 {
        ContextLevel::Critical
    } else if percent > 70.0 {
        ContextLevel::Warn
    } else {
        ContextLevel::Normal
    };
    let tenths = (percent * 10.0).round() as u64;
    let whole = tenths / 10;
    let frac = tenths % 10;
    let pct = if frac == 0 {
        format!("{whole}")
    } else {
        format!("{whole}.{frac}")
    };
    (format!("{pct}%/{}", format_tokens(window)), level)
}

/// Compose footer line 2 as its three display parts (ticket 04): the dim
/// stats block, the context segment (own text, styled by level at render
/// time), and the dim right-aligned model (with its left padding, possibly
/// truncated). The context segment rides with the stats on the left; on a
/// narrow terminal the model truncates first, then the stats+context block
/// truncates (pi truncates the stats side last). Returns `(stats, context,
/// model)`.
///
/// `context` is the segment's own text (as produced by [`context_part`])
/// including the leading separator it carries when the stats block is
/// non-empty. `None` hides the segment entirely (the SessionChanged→first-
/// Usage transition, ticket 05). When the stats+context block fills the line
/// the context segment is folded into the dim stats and returned as `None`,
/// losing its separate color in that extreme case.
pub fn stats_line_with_context(
    stats: &[String],
    context: Option<&str>,
    model: &str,
    width: usize,
) -> (String, Option<String>, String) {
    let stats_text = stats.join(" ");
    // The segment's own text (with its leading separator when the stats block
    // is non-empty), so the renderer can style it by level.
    let ctx_segment = context.map(|ctx| {
        if stats_text.is_empty() {
            ctx.to_string()
        } else {
            format!(" {ctx}")
        }
    });
    let ctx_width = ctx_segment.as_deref().map(display_width).unwrap_or(0);
    let left_width = display_width(&stats_text) + ctx_width;
    if left_width >= width {
        // The stats+context block fills the line: drop the model and truncate
        // the block. The context segment loses its separate color in this
        // extreme case (it is part of the dim truncated stats).
        let left = format!("{stats_text}{}", ctx_segment.as_deref().unwrap_or(""));
        return (truncate_width_str(&left, width), None, String::new());
    }
    let min_padding = 2;
    let total_needed = left_width + min_padding + display_width(model);
    if total_needed <= width {
        let padding = " ".repeat(width - left_width - display_width(model));
        return (stats_text, ctx_segment, format!("{padding}{model}"));
    }
    let available = width - left_width - min_padding;
    if available > 0 {
        let truncated = truncate_width_str(model, available);
        let pad = " ".repeat(width - left_width - display_width(&truncated));
        (stats_text, ctx_segment, format!("{pad}{truncated}"))
    } else {
        (stats_text, ctx_segment, String::new())
    }
}

/// Truncate `text` to `width` display columns, cutting at a char boundary so
/// wide (CJK) chars never split in half. Exposed so the `App` can truncate
/// footer line 1 (the dim pwd line) the same way the stats line is truncated.
pub fn truncate_width_str(text: &str, width: usize) -> String {
    truncate_width(text, width, false)
}

/// [`truncate_width_str`] with the cut marked by a trailing `…` (which takes
/// one of the `width` columns); used by the session picker's row titles. A
/// zero width yields an empty string — there is no room even for the marker.
pub fn truncate_width_ellipsis(text: &str, width: usize) -> String {
    truncate_width(text, width, true)
}

fn truncate_width(text: &str, width: usize, ellipsis: bool) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_string();
    }
    let budget = if ellipsis { width - 1 } else { width };
    let mut out = String::new();
    let mut w = 0;
    for ch in text.chars() {
        let cw = display_width(&ch.to_string());
        if w + cw > budget {
            break;
        }
        out.push(ch);
        w += cw;
    }
    if ellipsis {
        out.push('…');
    }
    out
}

/// Display width of a string in terminal columns (CJK wide chars count 2).
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_ports_pi_boundaries() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1000), "1.0k");
        assert_eq!(format_tokens(1500), "1.5k");
        assert_eq!(format_tokens(9999), "10.0k");
        assert_eq!(format_tokens(10_000), "10k");
        assert_eq!(format_tokens(999_999), "1000k");
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(1_500_000), "1.5M");
        assert_eq!(format_tokens(9_999_999), "10.0M");
        assert_eq!(format_tokens(10_000_000), "10M");
    }

    #[test]
    fn format_cwd_for_footer_shortens_home() {
        assert_eq!(
            format_cwd_for_footer("/home/u/proj", Some("/home/u")),
            "~/proj"
        );
        assert_eq!(format_cwd_for_footer("/home/u", Some("/home/u")), "~");
        // Outside home: unchanged.
        assert_eq!(format_cwd_for_footer("/opt/x", Some("/home/u")), "/opt/x");
        // No home env: unchanged.
        assert_eq!(format_cwd_for_footer("/home/u/proj", None), "/home/u/proj");
        // Home with trailing slash still matches.
        assert_eq!(
            format_cwd_for_footer("/home/u/proj", Some("/home/u/")),
            "~/proj"
        );
        // Sibling that only *prefixes* home must not shorten.
        assert_eq!(
            format_cwd_for_footer("/home/ux", Some("/home/u")),
            "/home/ux"
        );
    }

    #[test]
    fn stats_parts_omit_zeros_and_hit_rate() {
        let none = FooterUsage::default();
        assert!(stats_parts(&none).is_empty());

        let u = FooterUsage {
            input: 1500,
            output: 500,
            cache_read: 0,
            cache_write: 0,
        };
        assert_eq!(stats_parts(&u), vec!["↑1.5k", "↓500"]);

        let u = FooterUsage {
            input: 10_000,
            output: 0,
            cache_read: 8000,
            cache_write: 2000,
        };
        // pi `formatTokens`: 8000 → "8.0k" (one-decimal under 10k).
        // 8000/10000 = 80%
        assert_eq!(stats_parts(&u), vec!["↑10k", "R8.0k", "W2.0k", "CH80%"]);
    }

    #[test]
    fn cache_hit_percent_matches_rounding() {
        let u = FooterUsage {
            input: 3000,
            output: 0,
            cache_read: 1000,
            cache_write: 0,
        };
        // 1000/3000 = 33.33...% → 33.3%
        assert_eq!(cache_hit_percent(&u), Some("33.3".to_string()));
        let whole = FooterUsage {
            input: 4000,
            output: 0,
            cache_read: 2000,
            cache_write: 0,
        };
        assert_eq!(cache_hit_percent(&whole), Some("50".to_string()));
        let no_prompt = FooterUsage {
            input: 0,
            output: 10,
            cache_read: 5,
            cache_write: 0,
        };
        assert_eq!(cache_hit_percent(&no_prompt), None);
    }

    #[test]
    fn stats_line_right_aligns_model() {
        let stats = vec!["↑1.5k".to_string(), "↓500".to_string()];
        // Joined stats width = 10 (↑/↓ are 1 column each in unicode-width).
        // width 20: padding = 20 - 10 - 7 = 3.
        assert_eq!(stats_line(&stats, "model-x", 20), "↑1.5k ↓500   model-x");
        // Tight width truncates the model to the available columns.
        let line = stats_line(&stats, "model-very-long-name", 14);
        // 14 - 10 - 2 = 2 columns for the model → "mo".
        assert_eq!(line, "↑1.5k ↓500  mo");
        // Narrower than stats: stats alone (truncated to width).
        let line = stats_line(&stats, "m", 6);
        assert_eq!(line, "↑1.5k ");
    }

    #[test]
    fn truncate_width_marks_the_ellipsis_inside_the_budget() {
        // Fits: untouched, no marker.
        assert_eq!(truncate_width_ellipsis("short", 10), "short");
        assert_eq!(truncate_width_ellipsis("abcde", 5), "abcde");
        // Cut: the marker takes one of the columns.
        assert_eq!(truncate_width_ellipsis("abcdef", 5), "abcd…");
        assert_eq!(display_width(&truncate_width_ellipsis("abcdef", 5)), 5);
        // Wide chars never split, and the marker still fits the budget.
        assert_eq!(truncate_width_ellipsis("你好世界", 5), "你好…");
        // No room at all: empty, not a lone marker.
        assert_eq!(truncate_width_ellipsis("abc", 0), "");
        // The plain variant is the same cut without the marker.
        assert_eq!(truncate_width_str("abcdef", 5), "abcde");
    }

    #[test]
    fn new_stores_the_four_counters() {
        let f = FooterUsage::new(10, 5, 8, 2);
        assert_eq!(f.input, 10);
        assert_eq!(f.output, 5);
        assert_eq!(f.cache_read, 8);
        assert_eq!(f.cache_write, 2);
    }

    #[test]
    fn context_part_formats_percent_and_window() {
        // `45.3%/200k` — one decimal, trailing `.0` trimmed.
        assert_eq!(
            context_part(45.3, 200_000),
            ("45.3%/200k".to_string(), ContextLevel::Normal)
        );
        assert_eq!(
            context_part(0.0, 200_000),
            ("0%/200k".to_string(), ContextLevel::Normal)
        );
        // Levels: >70 warn, >90 critical.
        assert_eq!(context_part(70.0, 200_000).1, ContextLevel::Normal);
        assert_eq!(context_part(70.1, 200_000).1, ContextLevel::Warn);
        assert_eq!(context_part(90.0, 200_000).1, ContextLevel::Warn);
        assert_eq!(context_part(90.1, 200_000).1, ContextLevel::Critical);
        assert_eq!(context_part(99.9, 200_000).1, ContextLevel::Critical);
        // Rounding trims `.0` (45.04 → 45.0 → "45%", 45.05 → 45.1).
        assert_eq!(context_part(45.04, 200_000).0, "45%/200k");
        assert_eq!(context_part(45.05, 200_000).0, "45.1%/200k");
        // Window formatting reuses format_tokens.
        assert_eq!(context_part(1.5, 128_000).0, "1.5%/128k");
    }

    #[test]
    fn stats_line_with_context_places_the_segment_after_stats() {
        let stats = vec!["↑1.5k".to_string()];
        let ctx = "45.3%/200k";
        // `↑1.5k` (5) + ` 45.3%/200k` (11) = 16; width 40 → model right-aligned.
        let (s, c, m) = stats_line_with_context(&stats, Some(ctx), "model-x", 40);
        assert_eq!(s, "↑1.5k");
        assert_eq!(c.as_deref(), Some(" 45.3%/200k"));
        assert_eq!(m, "                 model-x");
        // No stats: the segment leads without a separator.
        let (s, c, _) = stats_line_with_context(&[], Some(ctx), "model-x", 40);
        assert_eq!(s, "");
        assert_eq!(c.as_deref(), Some("45.3%/200k"));
        // No context: the segment is absent (SessionChanged transition).
        let (s, c, m) = stats_line_with_context(&stats, None, "model-x", 40);
        assert_eq!(s, "↑1.5k");
        assert_eq!(c, None);
        assert!(m.trim_end().ends_with("model-x"));
    }

    #[test]
    fn stats_line_with_context_truncates_model_then_stats_block() {
        let stats = vec!["↑1.5k".to_string()];
        let ctx = "45.3%/200k";
        // Tight: model truncates first (available = 24 - 16 - 2 = 6).
        let (s, c, m) = stats_line_with_context(&stats, Some(ctx), "model-very-long", 24);
        assert_eq!(s, "↑1.5k");
        assert_eq!(c.as_deref(), Some(" 45.3%/200k"));
        assert!(m.starts_with(" "));
        assert_eq!(m.trim_start(), "model-");
        // Narrower than the block: block truncates, model dropped, and the
        // segment loses its separate color (None).
        let (s, c, m) = stats_line_with_context(&stats, Some(ctx), "m", 12);
        assert_eq!(s, "↑1.5k 45.3%/");
        assert_eq!(c, None);
        assert_eq!(m, "");
    }
}
