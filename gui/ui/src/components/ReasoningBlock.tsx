import { useState } from "react";
import { truncateLine } from "../format";

// A dim, collapsible reasoning block (spec user story 4): thinking content
// is visually separated from the formal answer. The header titles the
// truncated first line; the full text shows when expanded.

export function ReasoningBlock({ text }: { text: string }) {
  const [collapsed, setCollapsed] = useState(true);
  if (!text.trim()) return null;
  const preview = truncateLine(text, 48);
  return (
    <div className="mb-2">
      <button
        type="button"
        onClick={() => setCollapsed((c) => !c)}
        className="flex w-full items-center gap-1.5 text-xs font-medium text-slate-400 hover:text-slate-600"
      >
        <svg
          className={`size-3 transition-transform ${collapsed ? "" : "rotate-90"}`}
          viewBox="0 0 20 20"
          fill="currentColor"
        >
          <path
            fillRule="evenodd"
            d="M7.21 14.77a.75.75 0 0 1 .02-1.06L11.168 10 7.23 6.29a.75.75 0 1 1 1.04-1.08l4.5 4.25a.75.75 0 0 1 0 1.08l-4.5 4.25a.75.75 0 0 1-1.06-.02Z"
            clipRule="evenodd"
          />
        </svg>
        <span className="shrink-0">Thinking</span>
        <span className="min-w-0 truncate font-mono font-normal">
          {preview}
        </span>
      </button>
      {!collapsed ? (
        <pre className="mt-1 whitespace-pre-wrap font-mono text-xs leading-relaxed text-slate-400">
          {text}
        </pre>
      ) : null}
    </div>
  );
}
