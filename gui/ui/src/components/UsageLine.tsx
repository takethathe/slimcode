import type { TokenUsage } from "../types";

// The token-usage line rendered at the end of a turn (spec user story 7),
// mirroring the shared `usage_summary` wording of the CLI/TUI footer.

function cacheHitPercent(u: TokenUsage): string {
  const prompt = u.prompt_tokens;
  if (prompt === 0) return "0%";
  const cached = u.prompt_tokens_details?.cached_tokens ?? 0;
  const tenths = Math.round((cached * 1000) / prompt);
  const whole = Math.floor(tenths / 10);
  const frac = tenths % 10;
  return frac === 0 ? `${whole}%` : `${whole}.${frac}%`;
}

export function UsageLine({ usage }: { usage: TokenUsage }) {
  return (
    <div className="px-1 py-1 font-mono text-xs text-slate-400">
      tokens: {usage.prompt_tokens} prompt ({usage.prompt_tokens_details?.cached_tokens ?? 0} cached,{" "}
      {cacheHitPercent(usage)}) + {usage.completion_tokens} completion ={" "}
      {usage.total_tokens} total
    </div>
  );
}
