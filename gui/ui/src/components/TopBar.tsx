import type { ContextInfo } from "../types";
import { formatTokens } from "../format";

// The top bar (spec user story 15, ticket 06): session title, model name,
// and the context-window usage badge with threshold coloring (≤70% normal /
// >70% warn / >90% critical — the CLI footer's thresholds, ADR-0020).

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

export function TopBar({
  title,
  model,
  context,
  onCompact,
  compacting,
}: {
  title: string | null;
  model: string;
  context: ContextInfo | null;
  onCompact: () => void;
  compacting: boolean;
}) {
  const level = context ? contextLevel(context.percent) : "normal";
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
      {context ? (
        <span
          className={`inline-flex items-center rounded-full px-2.5 py-1 font-mono text-xs font-medium ring-1 ring-inset ${LEVEL_STYLES[level]}`}
          title={`Estimated context usage of ${formatTokens(context.window)} token window`}
        >
          {formatPercent(context.percent)}/{formatTokens(context.window)}
        </span>
      ) : null}
    </header>
  );
}
