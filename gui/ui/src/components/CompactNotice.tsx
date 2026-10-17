import { useState } from "react";
import { formatTokens } from "../format";

// A collapsible compaction notice (spec user story 8): the history was
// replaced by a summary; show the token change (before → after).

export function CompactNotice({
  before,
  after,
}: {
  before: number;
  after: number | null;
}) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="rounded-lg border border-amber-200 bg-amber-50/60 px-3 py-2">
      <button
        type="button"
        onClick={() => setExpanded((e) => !e)}
        className="flex w-full items-center gap-2 text-left text-xs font-medium text-amber-800"
      >
        <svg className="size-3.5" viewBox="0 0 20 20" fill="currentColor">
          <path
            fillRule="evenodd"
            d="M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495ZM10 5a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 10 5Zm0 9a1 1 0 1 0 0-2 1 1 0 0 0 0 2Z"
            clipRule="evenodd"
          />
        </svg>
        History compacted
        <span className="ml-auto font-mono text-amber-600">
          {after === null
            ? `${formatTokens(before)} tokens at compaction`
            : `${formatTokens(before)} → ${formatTokens(after)} tokens`}
        </span>
        <svg
          className={`size-3 transition-transform ${expanded ? "rotate-90" : ""}`}
          viewBox="0 0 20 20"
          fill="currentColor"
        >
          <path
            fillRule="evenodd"
            d="M7.21 14.77a.75.75 0 0 1 .02-1.06L11.168 10 7.23 6.29a.75.75 0 1 1 1.04-1.08l4.5 4.25a.75.75 0 0 1 0 1.08l-4.5 4.25a.75.75 0 0 1-1.06-.02Z"
            clipRule="evenodd"
          />
        </svg>
      </button>
      {expanded ? (
        <p className="mt-1 text-xs text-amber-700/80">
          The oldest part of this conversation was summarized to keep the
          context window within its limits. The summary is what the model now
          sees instead of the replaced span.
        </p>
      ) : null}
    </div>
  );
}
