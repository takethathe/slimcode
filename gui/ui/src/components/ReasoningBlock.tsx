import { useState } from "react";

// A dim, collapsible reasoning block (spec user story 4): thinking content
// is visually separated from the formal answer.

export function ReasoningBlock({ text }: { text: string }) {
  const [collapsed, setCollapsed] = useState(true);
  if (!text.trim()) return null;
  return (
    <div className="mb-2">
      <button
        type="button"
        onClick={() => setCollapsed((c) => !c)}
        className="flex items-center gap-1.5 text-xs font-medium text-slate-400 hover:text-slate-600"
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
        {collapsed ? "Thinking…" : "Thinking"}
      </button>
      {!collapsed ? (
        <pre className="mt-1 whitespace-pre-wrap font-mono text-xs leading-relaxed text-slate-400">
          {text}
        </pre>
      ) : null}
    </div>
  );
}
