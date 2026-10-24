// The pure Usage HUD module: cumulative accounting, the latest-request cache
// hit rate, and the tooltip / summary text. No React, no DOM — the whole HUD's
// visible numbers are derived here so they can be unit-tested against the Rust
// `usage_summary` / `cache_hit_percent` vectors (spec D8: the tooltip's summary
// sentence must stay byte-identical to the TUI/CLI wording).

import type { TokenUsage } from "./types";
import { formatTokens } from "./format";

/** A zeroed usage: the fresh-session HUD state (spec D4, empty `↑0 ↓0`). */
export const EMPTY_USAGE: TokenUsage = {
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  prompt_tokens_details: null,
};

/** Cache-hit prompt tokens; `0` when the endpoint omitted the details.
 *  Mirrors Rust `TokenUsage::cached_tokens`. */
export function cachedTokens(u: TokenUsage): number {
  return u.prompt_tokens_details?.cached_tokens ?? 0;
}

/** Cache-creation prompt tokens; `0` when details are absent.
 *  Mirrors Rust `TokenUsage::cache_creation_tokens`. */
export function cacheCreationTokens(u: TokenUsage): number {
  return u.prompt_tokens_details?.cache_creation_input_tokens ?? 0;
}

/** Cache hit percent of the prompt tokens (`cached / prompt`), rounded to one
 *  decimal and trimmed of a trailing `.0` (e.g. `68%`, `33.3%`); `0%` for a
 *  zero prompt or no cache activity. Integer tenths-of-a-percent math mirrors
 *  Rust `cache_hit_percent` so the summary wording cannot drift. */
export function cacheHitPercent(u: TokenUsage): string {
  const prompt = u.prompt_tokens;
  if (prompt === 0) return "0%";
  const tenths = Math.floor((cachedTokens(u) * 1000 + Math.floor(prompt / 2)) / prompt);
  const whole = Math.floor(tenths / 10);
  const frac = tenths % 10;
  return frac === 0 ? `${whole}%` : `${whole}.${frac}%`;
}

/** The `CH` badge text for the most recent request. `—` marks "unknown" (no
 *  request yet, the endpoint omitted cache details, or a zero-prompt request);
 *  otherwise the same percentage the summary uses — so `0%` means "details
 *  present but nothing cached", distinct from the unknown dash (spec D4). */
export function cacheHitBadge(latest: TokenUsage | null): string {
  if (latest === null) return "—";
  if (latest.prompt_tokens === 0) return "—";
  if (latest.prompt_tokens_details === null) return "—";
  return cacheHitPercent(latest);
}

/** The cumulative summary sentence, verbatim with the TUI/CLI `usage_summary`. */
export function usageSummary(u: TokenUsage): string {
  return (
    `tokens: ${u.prompt_tokens} prompt (${cachedTokens(u)} cached, ${cacheHitPercent(u)})` +
    ` + ${u.completion_tokens} completion = ${u.total_tokens} total`
  );
}

/** Field-wise addition merging cache details (absence counts as zero), the
 *  mirror of Rust `TokenUsage::saturating_add`. Used to fold one request's
 *  usage into the running session total between snapshot recalibrations. */
export function addUsage(a: TokenUsage, b: TokenUsage): TokenUsage {
  const details =
    a.prompt_tokens_details === null && b.prompt_tokens_details === null
      ? null
      : {
          cached_tokens: cachedTokens(a) + cachedTokens(b),
          cache_creation_input_tokens: cacheCreationTokens(a) + cacheCreationTokens(b),
        };
  return {
    prompt_tokens: a.prompt_tokens + b.prompt_tokens,
    completion_tokens: a.completion_tokens + b.completion_tokens,
    total_tokens: a.total_tokens + b.total_tokens,
    prompt_tokens_details: details,
  };
}

/** The HUD hover tooltip: the badge's `latest CH` rate first (so the reader
 *  knows that percentage is the last request, not the summary's cumulative
 *  rate), then cumulative cache read / write counts, then the full cumulative
 *  summary (spec D1/D8). */
export function usageTooltip(cumulative: TokenUsage, latest: TokenUsage | null): string {
  return (
    `latest CH${cacheHitBadge(latest)} · ` +
    `R${formatTokens(cachedTokens(cumulative))} W${formatTokens(cacheCreationTokens(cumulative))} · ` +
    usageSummary(cumulative)
  );
}
