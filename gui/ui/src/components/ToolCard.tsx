import { useState } from "react";
import type { ToolStatus } from "../transcript";
import { truncateLine } from "../format";

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

// Extract the shell command from a bash tool's JSON arguments, or `null`
// when the payload doesn't carry one.
function bashCommand(argumentsRaw: string): string | null {
  try {
    const parsed = JSON.parse(argumentsRaw) as { command?: unknown };
    return typeof parsed.command === "string" && parsed.command.length > 0
      ? parsed.command
      : null;
  } catch {
    return null;
  }
}

type ToolArgs = Record<string, unknown>;

function str(args: ToolArgs, key: string): string | null {
  const value = args[key];
  return typeof value === "string" && value.length > 0 ? value : null;
}

// The header's content preview per tool: bash shows the command, file
// tools (read/write/edit/ls) the path, grep/find pattern + path; unknown
// tools have no preview and keep the bare name as the title.
function titlePreview(
  name: string,
  argumentsRaw: string,
): string | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(argumentsRaw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const args = parsed as ToolArgs;
  switch (name) {
    case "bash":
      return str(args, "command");
    case "read":
    case "write":
    case "edit":
    case "ls":
      return str(args, "path");
    case "grep": {
      const pattern = str(args, "pattern");
      if (!pattern) return str(args, "path");
      const path = str(args, "path");
      return path ? `"${pattern}" in ${path}` : `"${pattern}"`;
    }
    case "find": {
      const parts: string[] = [];
      const path = str(args, "path");
      if (path) parts.push(path);
      const pattern = str(args, "pattern");
      if (pattern) parts.push(`"${pattern}"`);
      return parts.length > 0 ? parts.join(" ") : null;
    }
    default:
      return null;
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
  // The header titles the tool's key argument (truncated); the full
  // arguments stay available in the expanded view.
  const command = name === "bash" ? bashCommand(argumentsRaw) : null;
  const preview = titlePreview(name, argumentsRaw);
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
        {preview ? (
          <>
            <span className="shrink-0 font-mono text-xs font-medium uppercase tracking-wide text-slate-500">
              {name}
            </span>
            <span className="min-w-0 truncate font-mono text-sm font-medium text-slate-800">
              {truncateLine(preview, 60)}
            </span>
          </>
        ) : (
          <span className="font-mono text-sm font-medium text-slate-800">
            {name}
          </span>
        )}
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
          {command ? (
            <div>
              <div className="text-xs font-medium text-slate-500 mb-1">
                Command
              </div>
              <pre className="overflow-x-auto whitespace-pre-wrap rounded bg-slate-50 px-2 py-1.5 font-mono text-xs text-slate-700">
                {command}
              </pre>
            </div>
          ) : argumentsRaw ? (
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
