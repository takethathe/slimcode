// The Card view's reducer (ADR-0030 D3/D4/D8): card events in, per-Card Spec
// out. Pure — the same input sequence always yields the same panel state — so
// the fragment framing, bad-line tolerance, multi-Card grouping and the
// failure fallback are all testable without React.
//
// The op payload is a raw text fragment from the Card generator (it may be
// half a line: the provider streams by tokens). Both sides frame the stream
// the same way — split on "\n", keep the incomplete tail, flush it when the
// generation ends — so the webview and the host's mirror converge on the same
// Spec.

import {
  applySpecStreamPatch,
  parseSpecStreamLine,
  type Spec,
} from "@json-render/core";

/** One Card as the host's snapshot describes it. */
export type CardSnapshot = {
  id: string;
  kind: string;
  spec: Spec;
  /** Host-side recency counter; the array arrives most-recent-first. */
  updated: number;
};

/** The four flat events of the Card channel (ADR-0030 D4). */
export type CardEvent =
  | { kind: "card_begin"; card_id: string }
  | { kind: "card_op"; card_id: string; op: string }
  | { kind: "card_end"; card_id: string; ok: boolean }
  | { kind: "cards_snapshot"; cards: CardSnapshot[] };

export type CardState = {
  id: string;
  kind: string;
  spec: Spec;
  /** Lines applied since this generation began (drives the `ok` contract). */
  applied: number;
  generating: boolean;
  /** At least one generation applied nothing; the retry affordance shows. */
  failed: boolean;
  /** The spec shown before this generation began, restored on a dead end. */
  previous: Spec | null;
  /** The incomplete trailing line kept between fragments. */
  buffer: string;
};

export type CardsState = {
  /** Most recently generated first (ADR-0030 D2). */
  cards: CardState[];
};

/** The panel keeps at most this many Cards, newest first (ADR-0030 D2). */
export const MAX_CARDS = 3;

/** The ops json-render understands; anything else is a malformed line. */
const KNOWN_OPS = new Set(["add", "replace", "remove", "move", "copy", "test"]);

export function emptySpec(): Spec {
  return { root: "", elements: {} };
}

export function emptyCardsState(): CardsState {
  return { cards: [] };
}

/**
 * Apply a batch of fragments to `spec`, returning a new spec plus the number
 * of complete lines that applied. Bad lines are dropped one at a time: a
 * model's malformed line never costs the lines around it.
 */
export function applyFragments(
  spec: Spec,
  buffer: string,
  fragments: string[],
): { spec: Spec; buffer: string; applied: number } {
  let next: Spec = cloneSpec(spec);
  let tail = buffer;
  let applied = 0;
  for (const fragment of fragments) {
    tail += fragment;
    const lines = tail.split("\n");
    tail = lines.pop() ?? "";
    for (const line of lines) {
      if (applyLine(next, line)) applied += 1;
    }
  }
  return { spec: next, buffer: tail, applied };
}

/** Apply whatever is left in the buffer (the stream ended mid-line). */
export function flushBuffer(spec: Spec, buffer: string): { spec: Spec; applied: number } {
  const trimmed = buffer.trim();
  if (!trimmed) return { spec, applied: 0 };
  const next = cloneSpec(spec);
  return { spec: next, applied: applyLine(next, trimmed) ? 1 : 0 };
}

function applyLine(spec: Spec, line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed) return false;
  const patch = parseSpecStreamLine(trimmed);
  if (!patch) return false;
  // An unknown op or an empty pointer is a malformed line: the host drops it
  // too, and the two sides must agree on which lines "applied".
  if (!KNOWN_OPS.has(patch.op) || patch.path === "") return false;
  try {
    applySpecStreamPatch(spec as unknown as Record<string, unknown>, patch);
    return true;
  } catch {
    // A `test` op that fails, or a structure the patch cannot reach: drop the
    // line and keep the rest of the stream.
    return false;
  }
}

function cloneSpec(spec: Spec): Spec {
  return JSON.parse(JSON.stringify(spec)) as Spec;
}

/** Reduce one ordered batch of Card events. */
export function reduceCards(state: CardsState, events: CardEvent[]): CardsState {
  let cards = state.cards;
  for (const event of events) {
    switch (event.kind) {
      case "cards_snapshot": {
        cards = event.cards
          .map((card) => ({
            id: card.id,
            kind: card.kind,
            spec: card.spec,
            applied: 0,
            generating: false,
            failed: false,
            previous: null,
            buffer: "",
          }))
          .slice(0, MAX_CARDS);
        break;
      }
      case "card_begin": {
        const existing = cards.find((card) => card.id === event.card_id);
        const began: CardState = {
          id: event.card_id,
          kind: event.card_id,
          spec: emptySpec(),
          applied: 0,
          generating: true,
          failed: false,
          previous: existing ? existing.spec : null,
          buffer: "",
        };
        cards = [began, ...cards.filter((card) => card.id !== event.card_id)].slice(
          0,
          MAX_CARDS,
        );
        break;
      }
      case "card_op": {
        const index = cards.findIndex((card) => card.id === event.card_id);
        // An op for a Card this panel does not know yet (a reload raced the
        // first begin) is ignored rather than inventing a Card.
        if (index < 0) break;
        const card = cards[index];
        const { spec, buffer, applied } = applyFragments(card.spec, card.buffer, [
          event.op,
        ]);
        cards = replaceAt(cards, index, {
          ...card,
          spec,
          buffer,
          applied: card.applied + applied,
          generating: true,
        });
        break;
      }
      case "card_end": {
        const index = cards.findIndex((card) => card.id === event.card_id);
        if (index < 0) break;
        const card = cards[index];
        const flushed = flushBuffer(card.spec, card.buffer);
        const applied = card.applied + flushed.applied;
        // D8: a generation that applied nothing keeps the previous Card
        // visible and asks for a retry in the panel.
        const keepPrevious = !event.ok && applied === 0 && card.previous !== null;
        cards = replaceAt(cards, index, {
          ...card,
          spec: keepPrevious ? card.previous! : flushed.spec,
          buffer: "",
          applied,
          generating: false,
          failed: !event.ok,
          previous: null,
        });
        break;
      }
    }
  }
  return { cards };
}

function replaceAt(cards: CardState[], index: number, card: CardState): CardState[] {
  const next = [...cards];
  next[index] = card;
  return next;
}

/** Whether any Card is currently generating (the single-flight feedback). */
export function isGenerating(state: CardsState): boolean {
  return state.cards.some((card) => card.generating);
}

/** Whether any Card is showing the retry affordance. */
export function hasFailure(state: CardsState): boolean {
  return state.cards.some((card) => card.failed);
}

/** The human label of a Card kind (the panel's chip). */
export function cardKindLabel(kind: string): string {
  switch (kind) {
    case "task-board":
      return "Task board";
    case "result":
      return "Result";
    case "insight":
      return "Insight";
    default:
      return kind;
  }
}
