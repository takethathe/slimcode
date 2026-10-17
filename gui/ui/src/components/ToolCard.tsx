import { useState } from "react";
import type { ToolStatus } from "../transcript";

// A tool invocation card: name + status badge, with the raw JSON arguments
// and the tool's result collapsed by default (spec user stories 5-6).

const STATUS_STYLES: Record<ToolStatus, string> = {
  running:
    "bg-indigo-50 text-indigo-700 ring-indigo-200 animate-pulse",
  ok: "bg-emerald-50 text-emerald-700 ring-emerald-200",
  error: "bg-red-50 text-red-700 ring-red-200",
};

const STATUS_LABELS: Record<ToolStatus, string> = {
  running: "running",
  ok: "ok",
  error: "failed",
};

function StatusBadge({ status }: { status: ToolStatus }) {
  return (
    <span
      className={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium ring-1 ring-inset ${STATUS_STYLES[status]}`}
    >
      {status === "running" ? (
        <span className="mr-1.5 size-1.5 rounded-full bg-indigo-500 animate-pulse" />
      ) : null}
      {STATUS_LABELS[status]}
    </span>
  );
}

function prettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

export function ToolCard({
  name,
  status,
  argumentsRaw,
  result,
}: {
  name: string;
  status: ToolStatus;
  argumentsRaw: string;
  result: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const border =
    status === "error"
      ? "border-red-200"
      : status === "ok"
        ? "border-emerald-200"
        : "border-indigo-200";

  return (
    <div className={`rounded-lg border ${border} bg-white shadow-sm`}>
      <button
        type="button"
        onClick={() => setExpanded((e) => !e)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left"
      >
        <span className="font-mono text-sm font-medium text-slate-800">
          {name}
        </span>
        <StatusBadge status={status} />
        <span className="ml-auto text-slate-400">
          <svg
            className={`size-4 transition-transform ${expanded ? "rotate-90" : ""}`}
            viewBox="0 0 20 20"
            fill="currentColor"
          >
            <path
              fillRule="evenodd"
              d="M7.21 14.77a.75.75 0 0 1 .02-1.06L11.168 10 7.23 6.29a.75.75 0 1 1 1.04-1.08l4.5 4.25a.75.75 0 0 1 0 1.08l-4.5 4.25a.75.75 0 0 1-1.06-.02Z"
              clipRule="evenodd"
            />
          </svg>
        </span>
      </button>
      {expanded ? (
        <div className="border-t border-slate-100 px-3 py-2 space-y-2">
          {argumentsRaw ? (
            <div>
              <div className="text-xs font-medium text-slate-500 mb-1">
                Arguments
              </div>
              <pre className="overflow-x-auto rounded bg-slate-50 px-2 py-1.5 font-mono text-xs text-slate-700">
                {prettyJson(argumentsRaw)}
              </pre>
            </div>
          ) : null}
          {result ? (
            <div>
              <div className="text-xs font-medium text-slate-500 mb-1">
                Result
              </div>
              <pre className="max-h-64 overflow-auto whitespace-pre-wrap rounded bg-slate-50 px-2 py-1.5 font-mono text-xs text-slate-700">
                {result}
              </pre>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
