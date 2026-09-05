//! Fuzzy subsequence matching for `/` completion prediction.
//!
//! `fuzzy_match` answers "does `query` appear as a (not necessarily
//! consecutive) subsequence of `text`?" and returns a score where **lower is
//! better**: consecutive runs, word-boundary hits and exact matches are
//! rewarded; gaps and late positions are penalised. The heuristic is adapted
//! from pi's `fuzzy.ts` so command/skill completion feels familiar, but kept
//! integer-scaled so it is deterministic and trivially testable.

/// Rewards and penalties, integer-scaled from pi's `fuzzy.ts` (×10 so the
/// 0.1-per-position penalty stays an integer). Lower is better.
const CONSECUTIVE_BONUS: i64 = 50; // per char after the first in a run
const WORD_BOUNDARY_BONUS: i64 = 100; // hit preceded by a word boundary
const EXACT_BONUS: i64 = 1000; // query exactly equals text
const GAP_PENALTY: i64 = 20; // per skipped char between hits
const POSITION_PENALTY: i64 = 1; // per index position of a hit

/// Characters that count as a word boundary immediately before a hit.
fn is_boundary(prev: Option<char>) -> bool {
    matches!(
        prev,
        None | Some(' ') | Some('-') | Some('_') | Some('.') | Some('/') | Some(':')
    )
}

/// Match `query` against `text` as a subsequence. Returns `Some(score)` when
/// every character of `query` appears in `text` in order, `None` otherwise.
///
/// Lower score is better. All characters are compared case-sensitively.
pub fn fuzzy_match(query: &str, text: &str) -> Option<i64> {
    let query: Vec<char> = query.chars().collect();
    let text: Vec<char> = text.chars().collect();
    if query.is_empty() {
        return Some(0);
    }
    if text.is_empty() {
        return None;
    }
    if query == text {
        return Some(-EXACT_BONUS);
    }

    // Greedy forward scan: match each query char at the earliest position
    // possible, scoring hits as we go. Greedy is not always globally optimal,
    // but it keeps the matcher simple and deterministic; for short command
    // names it matches what a human expects from a quick prefix/loose match.
    let mut score = 0i64;
    let mut ti = 0usize;
    let mut consecutive = 0usize;
    for &qc in &query {
        // Advance to the first occurrence of `qc` at or after the current
        // position (compare as `char` for code points, matching pi).
        let mut found = None;
        for (i, &tc) in text.iter().enumerate().skip(ti) {
            if tc == qc {
                found = Some(i);
                break;
            }
        }
        let i = found?;

        let gap = i.saturating_sub(ti);
        if gap == 0 {
            consecutive += 1;
        } else {
            consecutive = 0;
            score += GAP_PENALTY * gap as i64;
        }
        score += POSITION_PENALTY * i as i64;
        if is_boundary(text.get(i.wrapping_sub(1)).copied()) {
            score -= WORD_BOUNDARY_BONUS;
        }
        if consecutive > 1 {
            score -= CONSECUTIVE_BONUS;
        }
        ti = i + 1;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, text: &str) -> i64 {
        fuzzy_match(query, text).expect("query should match text")
    }

    #[test]
    fn empty_query_matches_anything_with_zero() {
        assert_eq!(fuzzy_match("", "anything"), Some(0));
        assert_eq!(fuzzy_match("", ""), Some(0));
    }

    #[test]
    fn empty_text_only_matches_empty_query() {
        assert_eq!(fuzzy_match("", ""), Some(0));
        assert_eq!(fuzzy_match("a", ""), None);
    }

    #[test]
    fn exact_equality_matches() {
        assert!(fuzzy_match("save", "save").is_some());
        assert_eq!(score("save", "save"), score("save", "save"));
    }

    #[test]
    fn non_subsequence_is_none() {
        assert_eq!(fuzzy_match("abc", "ac"), None);
        assert_eq!(fuzzy_match("zz", "z"), None);
        assert_eq!(fuzzy_match("xy", "yx"), None); // order matters
    }

    #[test]
    fn subsequence_matches() {
        assert!(fuzzy_match("sv", "save").is_some());
        assert!(fuzzy_match("install", "install-skill").is_some());
        assert!(fuzzy_match("ist", "install-skill").is_some()); // i,s,t in order
        assert!(fuzzy_match("sk", "skills").is_some());
    }

    #[test]
    fn case_sensitive() {
        assert_eq!(fuzzy_match("SAVE", "save"), None);
        assert_eq!(fuzzy_match("save", "SAVE"), None);
    }

    #[test]
    fn contiguous_run_is_better_than_gapped() {
        // "save" appears contiguously in "save"; in "sessions-and-validate"
        // the chars are spread out, so the former must score better.
        let contiguous = score("sv", "save");
        let gapped = score("sv", "sessions-and-validate");
        assert!(contiguous < gapped, "{contiguous} < {gapped}");
    }

    #[test]
    fn exact_is_best() {
        // An exact match outranks a gappy match of the same query.
        let exact = score("save", "save");
        let gappy = score("save", "sessions-are-validated-everywhere");
        assert!(exact < gappy, "{exact} < {gappy}");
    }

    #[test]
    fn word_boundary_bonus_beats_midword() {
        // "sh" hitting at a word boundary ("-sh" in "install-skill" is not a
        // boundary; "s" after "-" is) should beat an equal-length gap midword.
        let boundary = score("sk", "skills"); // "s" at start (boundary)
        let midword = score("sk", "ask"); // "s" midword, "k" after
        assert!(boundary < midword, "{boundary} < {midword}");
    }

    #[test]
    fn position_penalty_prefers_early_matches() {
        // Same query, same gaps; the earlier occurrence scores better.
        let early = score("hl", "helloworld");
        let late = score("hl", "zzhelloworld");
        assert!(early < late, "{early} < {late}");
    }

    #[test]
    fn longer_gap_is_worse() {
        let small_gap = score("sv", "sav");
        let large_gap = score("sv", "s----------v");
        assert!(small_gap < large_gap, "{small_gap} < {large_gap}");
    }
}
