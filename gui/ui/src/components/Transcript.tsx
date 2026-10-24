import { useEffect, useRef } from "react";
import type { Block } from "../transcript";
import { ToolCard } from "./ToolCard";
import { ReasoningBlock } from "./ReasoningBlock";
import { CompactNotice } from "./CompactNotice";
import { ChipBadge } from "./Chip";
import { Markdown } from "./Markdown";

// The transcript area: renders the block list with auto-follow scrolling —
// it sticks to the bottom while the user is near it and pauses while the
// user has scrolled up (spec user story 20, aligned with the TUI's
// Transcript mental model, ADR-0017).

const FOLLOW_THRESHOLD = 48;

export function Transcript({
  blocks,
  running,
}: {
  blocks: Block[];
  running: boolean;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (stickRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [blocks]);

  function onScroll(): void {
    const el = scrollRef.current;
    if (!el) return;
    stickRef.current =
      el.scrollHeight - el.scrollTop - el.clientHeight < FOLLOW_THRESHOLD;
  }

  return (
    <div
      ref={scrollRef}
      onScroll={onScroll}
      className="flex-1 overflow-y-auto px-6 py-4"
    >
      {blocks.length === 0 ? (
        <div className="flex h-full items-center justify-center">
          <div className="text-center">
            <div className="text-2xl font-semibold text-slate-400">
              {running ? "Working…" : "New session"}
            </div>
            <p className="mt-1 text-sm text-slate-400">
              Type a prompt below to start.
            </p>
          </div>
        </div>
      ) : (
        <div className="mx-auto max-w-3xl space-y-4">
          {blocks.map((block) => (
            <BlockView key={block.id} block={block} />
          ))}
          {running ? (
            <div className="flex items-center gap-2 text-sm text-indigo-500">
              <span className="size-1.5 rounded-full bg-indigo-500 animate-pulse" />
              Working…
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
}

// Transcript chip: an inline rounded badge wrapping a leading `/{token}`, the
// submitted-side sibling of the input-side Command chip. Colour comes from the
// shared ChipBadge (skill teal, command amber).
function BlockView({ block }: { block: Block }) {
  switch (block.kind) {
    case "user":
      return (
        <div className="rounded-lg bg-indigo-50 px-3 py-2">
          <p className="whitespace-pre-wrap break-words font-mono text-sm font-bold leading-relaxed text-indigo-900">
            {block.chip ? (
              <ChipBadge
                category={block.chip.category}
                token={block.chip.token}
                className="mr-1.5 align-middle"
              />
            ) : null}
            {block.text}
          </p>
        </div>
      );
    case "assistant":
      return (
        <div className="min-w-0">
          <ReasoningBlock text={block.reasoning} />
          {block.text ? <Markdown text={block.text} /> : null}
        </div>
      );
    case "skill":
      return (
        <div className="ml-9 border-l-4 border-indigo-400 bg-slate-100 px-3 py-2 font-mono text-sm text-slate-700">
          Skill "{block.name}" activated
        </div>
      );
    case "tool":
      return (
        <div className="ml-9">
          <ToolCard
            name={block.name}
            status={block.status}
            argumentsRaw={block.arguments}
            result={block.result}
          />
        </div>
      );
    case "compacted":
      return (
        <div className="ml-9">
          <CompactNotice before={block.before} after={block.after} />
        </div>
      );
    case "error":
      return (
        <div className="ml-9 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
          {block.text}
        </div>
      );
  }
}
