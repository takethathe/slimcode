// The right-hand Card view (ADR-0030): a fixed-width, collapsible panel that
// renders each Card independently. The Transcript and the panel scroll on
// their own; collapsing hands the width back to the conversation.
//
// The panel owns no Card truth: everything it paints comes from the reducer
// state (`cards.ts`), which comes from the host's event channel.

import type { CardState } from "../cards";
import { cardKindLabel } from "../cards";
import { CardRenderer } from "../cardRegistry";

export function CardView({
  cards,
  generating,
  collapsed,
  notice,
  onToggle,
  onGenerate,
  onPreview,
}: {
  cards: CardState[];
  generating: boolean;
  collapsed: boolean;
  notice: string | null;
  onToggle: () => void;
  onGenerate: () => void;
  /** Debug-only: replay a synthetic op stream in the panel (dev builds). */
  onPreview?: () => void;
}) {
  if (collapsed) {
    return (
      <aside
        aria-label="Cards"
        className="flex w-10 shrink-0 flex-col items-center border-l border-slate-200 bg-white py-2"
      >
        <button
          type="button"
          aria-label="Expand cards"
          aria-expanded={false}
          onClick={onToggle}
          className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700"
        >
          ‹
        </button>
        <span className="mt-3 select-none text-[0.65rem] font-medium uppercase tracking-wide text-slate-400">
          Cards
        </span>
        {cards.length > 0 ? (
          <span className="mt-1 rounded-full bg-indigo-50 px-1.5 py-0.5 text-[0.65rem] font-semibold text-indigo-600">
            {cards.length}
          </span>
        ) : null}
      </aside>
    );
  }

  return (
    <aside
      aria-label="Cards"
      className="flex w-[22rem] shrink-0 flex-col border-l border-slate-200 bg-slate-50"
    >
      <header className="flex items-center gap-2 border-b border-slate-200 px-3 py-2">
        <h2 className="flex-1 text-xs font-semibold uppercase tracking-wide text-slate-500">
          Cards
        </h2>
        {import.meta.env.DEV && onPreview ? (
          <button
            type="button"
            aria-label="Preview cards"
            onClick={onPreview}
            className="rounded px-1.5 py-0.5 text-xs text-slate-400 hover:bg-slate-100 hover:text-slate-700"
          >
            Preview
          </button>
        ) : null}
        <button
          type="button"
          aria-label={generating ? "Generating cards" : "Generate cards"}
          onClick={onGenerate}
          className="rounded px-1.5 py-0.5 text-xs font-medium text-indigo-600 hover:bg-indigo-50 disabled:text-slate-400"
        >
          {generating ? "Generating…" : "Generate"}
        </button>
        <button
          type="button"
          aria-label="Collapse cards"
          aria-expanded
          onClick={onToggle}
          className="rounded p-0.5 text-slate-400 hover:bg-slate-100 hover:text-slate-700"
        >
          ›
        </button>
      </header>
      {notice ? (
        <p className="border-b border-amber-200 bg-amber-50 px-3 py-1.5 text-xs text-amber-800">
          {notice}
        </p>
      ) : null}
      <div className="flex-1 space-y-3 overflow-y-auto px-3 py-3">
        {cards.length === 0 ? (
          <div className="flex h-full items-center justify-center">
            <div className="px-4 text-center">
              <p className="text-sm font-medium text-slate-400">No cards yet</p>
              <p className="mt-1 text-xs text-slate-400">
                A card appears after a turn completes.
              </p>
            </div>
          </div>
        ) : (
          cards.map((card) => (
            <article key={card.id} data-card-kind={card.id} className="space-y-1.5">
              <div className="flex items-center gap-2">
                <span className="rounded bg-slate-200/70 px-1.5 py-0.5 text-[0.65rem] font-semibold uppercase tracking-wide text-slate-600">
                  {cardKindLabel(card.kind)}
                </span>
                {card.generating ? (
                  <span className="text-[0.65rem] text-indigo-500">streaming</span>
                ) : null}
                {card.failed ? (
                  <button
                    type="button"
                    onClick={onGenerate}
                    className="ml-auto text-[0.65rem] font-medium text-red-600 underline"
                  >
                    Generation failed — retry
                  </button>
                ) : null}
              </div>
              {card.generating ? (
                <div
                  role="progressbar"
                  aria-label="Card generation progress"
                  className="h-0.5 overflow-hidden rounded bg-slate-200"
                >
                  <div className="h-full w-1/3 animate-pulse rounded bg-indigo-500" />
                </div>
              ) : null}
              {card.spec.root ? (
                <CardRenderer spec={card.spec} />
              ) : (
                <div className="rounded-lg border border-dashed border-slate-200 px-3 py-4 text-center text-xs text-slate-400">
                  {card.generating ? "Waiting for the first element…" : "Nothing rendered"}
                </div>
              )}
            </article>
          ))
        )}
      </div>
    </aside>
  );
}
