import { useEffect, useState } from "react";
import type { ContextInfo, TokenUsage } from "../types";
import { formatTokens } from "../format";
import { cacheHitBadge, usageTooltip } from "../usage";

// The top bar: session title, model name, the Compact action, and the
// resident Usage HUD — `↑ ↓ CH ctx` (spec .scratch/gui-usage-hud). The
// first three come from the usage module; the
// fourth is the context-window badge with threshold coloring (≤70% normal /
// >70% warn / >90% critical — the CLI footer's thresholds, ADR-0020). All four
// stay resident (zero values included) so their position never jumps, and the
// hover tooltip carries the cache read/write counts plus the full cumulative
// summary.

function formatPercent(percent: number): string {
  const tenths = Math.round(percent * 10);
  const whole = Math.floor(tenths / 10);
  const frac = tenths % 10;
  return frac === 0 ? `${whole}%` : `${whole}.${frac}%`;
}

function contextLevel(percent: number): "normal" | "warn" | "critical" {
  if (percent > 90) return "critical";
  if (percent > 70) return "warn";
  return "normal";
}

const LEVEL_STYLES = {
  normal: "bg-slate-100 text-slate-600 ring-slate-200",
  warn: "bg-amber-50 text-amber-700 ring-amber-200",
  critical: "bg-red-50 text-red-700 ring-red-200",
};

const BADGE =
  "inline-flex shrink-0 items-center rounded-full bg-slate-100 px-2 py-1 font-mono text-xs font-medium text-slate-600 ring-1 ring-inset ring-slate-200";

export function TopBar({
  title,
  model,
  context,
  onCompact,
  compacting,
  usage,
  latest,
  pulse,
}: {
  title: string | null;
  model: string;
  context: ContextInfo | null;
  onCompact: () => void;
  compacting: boolean;
  usage: TokenUsage;
  latest: TokenUsage | null;
  pulse: number;
}) {
  const level = context ? contextLevel(context.percent) : "normal";
  // A `/usage` bump rings the HUD once (~600ms) as feedback (spec D7). The CSS
  // class drives a one-shot pulse; under `prefers-reduced-motion` it degrades
  // to a static outline for the same window (see index.css).
  const [ringing, setRinging] = useState(false);
  useEffect(() => {
    if (pulse <= 0) return;
    setRinging(true);
    const timer = setTimeout(() => setRinging(false), 600);
    return () => clearTimeout(timer);
  }, [pulse]);
  return (
    <header className="flex h-14 shrink-0 items-center gap-3 border-b border-slate-200 bg-white px-4">
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-semibold text-slate-800">
          {title || "New session"}
        </div>
        <div className="truncate text-xs text-slate-400">
          {model || "No model configured"}
        </div>
      </div>
      <button
        type="button"
        onClick={onCompact}
        disabled={compacting}
        className="rounded-md border border-slate-200 px-2.5 py-1 text-xs font-medium text-slate-600 hover:bg-slate-50 disabled:opacity-50"
        title="Compact history now"
      >
        Compact
      </button>
      <div
        className={`flex shrink-0 items-center gap-1.5 rounded-full${ringing ? " usage-hud-active" : ""}`}
        title={usageTooltip(usage, latest)}
      >
        <span className={BADGE}>↑{formatTokens(usage.prompt_tokens)}</span>
        <span className={BADGE}>↓{formatTokens(usage.completion_tokens)}</span>
        <span className={BADGE}>CH{cacheHitBadge(latest)}</span>
        {context ? (
          <span
            className={`inline-flex shrink-0 items-center rounded-full px-2.5 py-1 font-mono text-xs font-medium ring-1 ring-inset ${LEVEL_STYLES[level]}`}
            title={`Estimated context usage of ${formatTokens(context.window)} token window`}
          >
            {formatPercent(context.percent)}/{formatTokens(context.window)}
          </span>
        ) : null}
      </div>
    </header>
  );
}
