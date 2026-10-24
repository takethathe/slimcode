// Tests for the Card view reducer (ADR-0030 D3/D4/D8): fragment framing, bad
// lines, multi-Card grouping, batch equivalence, the failure fallback, and the
// cross-language fixture that pins the webview and the host to the same Spec.

import { describe, expect, it } from "vitest";

import {
  applyFragments,
  cardKindLabel,
  emptyCardsState,
  hasFailure,
  isGenerating,
  reduceCards,
  type CardEvent,
  type CardState,
  type CardsState,
} from "./cards";
import fixture from "./fixtures/card-stream.json";

/** Fold a flat stream of (card, fragment) pairs into card events the way the
 *  host frames one generation: a begin per Card, then the ops, then the ends. */
function eventsFromFixture(stream: { card: string; op: string }[]): CardEvent[] {
  const order: string[] = [];
  for (const entry of stream) {
    if (!order.includes(entry.card)) order.push(entry.card);
  }
  return [
    ...order.map((card): CardEvent => ({ kind: "card_begin", card_id: card })),
    ...stream.map((entry): CardEvent => ({
      kind: "card_op",
      card_id: entry.card,
      op: entry.op,
    })),
    ...order.map((card): CardEvent => ({ kind: "card_end", card_id: card, ok: true })),
  ];
}

/** The one line of a Card stream that is live: a whole JSON patch per line. */
function line(patch: Record<string, unknown>): string {
  return `${JSON.stringify(patch)}\n`;
}

function card(spec: CardsState, id: string): CardState {
  const found = spec.cards.find((entry) => entry.id === id);
  if (!found) throw new Error(`no card ${id}`);
  return found;
}

describe("fixture", () => {
  it("converges on the expected Spec on both sides", () => {
    const state = reduceCards(emptyCardsState(), eventsFromFixture(fixture.stream));
    for (const [id, expected] of Object.entries(fixture.expected)) {
      expect(card(state, id).spec).toEqual(expected);
    }
    expect(state.cards.map((entry) => entry.id)).toEqual(["result", "task-board"]);
  });
});

describe("fragment framing", () => {
  it("holds a split line until the next fragment closes it", () => {
    const first = applyFragments({ root: "", elements: {} }, "", ['{"op":"add","path":"/root"']);
    expect(first.applied).toBe(0);
    expect(first.buffer).toBe('{"op":"add","path":"/root"');
    expect(first.spec).toEqual({ root: "", elements: {} });

    const second = applyFragments(first.spec, first.buffer, [',"value":"r"}\n']);
    expect(second.applied).toBe(1);
    expect(second.buffer).toBe("");
    expect(second.spec).toEqual({ root: "r", elements: {} });
  });

  it("applies several lines delivered in one fragment", () => {
    const batch = applyFragments({ root: "", elements: {} }, "", [
      `${line({ op: "add", path: "/root", value: "root" })}${line({
        op: "add",
        path: "/elements/root",
        value: { type: "Text", props: { content: "hi" }, children: [] },
      })}`,
    ]);
    expect(batch.applied).toBe(2);
    expect(batch.spec.root).toBe("root");
    expect(Object.keys(batch.spec.elements)).toEqual(["root"]);
  });

  it("drops a bad line without losing the lines around it", () => {
    const result = applyFragments({ root: "", elements: {} }, "", [
      line({ op: "add", path: "/root", value: "root" }),
      '{"op":"add","path":\n',
      "not json at all\n",
      line({ op: "add", path: "/kind", value: "result" }),
    ]);
    expect(result.applied).toBe(2);
    expect(result.spec).toMatchObject({ root: "root", kind: "result" });
  });

  it("drops a patch the structure cannot reach", () => {
    // `test` fails when the value differs: the line is dropped, the stream goes on.
    const result = applyFragments({ root: "", elements: {} }, "", [
      line({ op: "test", path: "/root", value: "expected" }),
      line({ op: "add", path: "/root", value: "root" }),
    ]);
    expect(result.applied).toBe(1);
    expect(result.spec.root).toBe("root");
  });

  it("drops an unknown op and an empty pointer like the host does", () => {
    const result = applyFragments({ root: "", elements: {} }, "", [
      line({ op: "merge", path: "/root", value: "r" }),
      line({ op: "add", path: "", value: "r" }),
      line({ op: "add", path: "/root", value: "root" }),
    ]);
    expect(result.applied).toBe(1);
    expect(result.spec).toEqual({ root: "root", elements: {} });
  });
});

describe("reduceCards", () => {
  it("keeps Cards apart when their ops interleave", () => {
    const state = reduceCards(emptyCardsState(), [
      { kind: "card_begin", card_id: "result" },
      { kind: "card_begin", card_id: "insight" },
      { kind: "card_op", card_id: "result", op: line({ op: "add", path: "/root", value: "r" }) },
      { kind: "card_op", card_id: "insight", op: line({ op: "add", path: "/root", value: "i" }) },
      { kind: "card_op", card_id: "result", op: line({ op: "add", path: "/kind", value: "result" }) },
    ]);
    expect(card(state, "result").spec).toMatchObject({ root: "r", kind: "result" });
    expect(card(state, "insight").spec).toMatchObject({ root: "i" });
    // Only the Card that saw the op carries the extra field.
    expect(card(state, "insight").spec).toEqual({ root: "i", elements: {} });
  });

  it("puts the most recent Card first and keeps one entry per kind", () => {
    const state = reduceCards(emptyCardsState(), [
      { kind: "card_begin", card_id: "result" },
      { kind: "card_begin", card_id: "insight" },
      { kind: "card_begin", card_id: "result" },
    ]);
    expect(state.cards.map((entry) => entry.id)).toEqual(["result", "insight"]);
    expect(state.cards[0].generating).toBe(true);
  });

  it("ignores an op for a Card it has not seen begin", () => {
    const state = reduceCards(emptyCardsState(), [
      { kind: "card_op", card_id: "result", op: line({ op: "add", path: "/root", value: "r" }) },
    ]);
    expect(state.cards).toEqual([]);
  });

  it("keeps at most three Cards, newest first", () => {
    const state = reduceCards(emptyCardsState(), [
      { kind: "card_begin", card_id: "result" },
      { kind: "card_begin", card_id: "insight" },
      { kind: "card_begin", card_id: "task-board" },
      { kind: "card_begin", card_id: "result" },
    ]);
    expect(state.cards.map((entry) => entry.id)).toEqual([
      "result",
      "task-board",
      "insight",
    ]);
  });

  it("flushes the trailing fragment when the generation ends", () => {
    const state = reduceCards(emptyCardsState(), [
      { kind: "card_begin", card_id: "result" },
      { kind: "card_op", card_id: "result", op: '{"op":"add","path":"/root","value":"r"}' },
      { kind: "card_end", card_id: "result", ok: true },
    ]);
    expect(card(state, "result").spec.root).toBe("r");
    expect(card(state, "result").buffer).toBe("");
    expect(card(state, "result").generating).toBe(false);
    expect(isGenerating(state)).toBe(false);
  });

  it("yields the same state for one batch as for one event at a time", () => {
    const events = eventsFromFixture(fixture.stream);
    const batched = reduceCards(emptyCardsState(), events);
    let stepped = emptyCardsState();
    for (const event of events) stepped = reduceCards(stepped, [event]);
    expect(batched).toEqual(stepped);
  });

  it("replaces everything from a snapshot", () => {
    const state = reduceCards(
      reduceCards(emptyCardsState(), [{ kind: "card_begin", card_id: "result" }]),
      [
        {
          kind: "cards_snapshot",
          cards: [
            {
              id: "insight",
              kind: "insight",
              spec: { root: "root", elements: {} },
              updated: 3,
            },
          ],
        },
      ],
    );
    expect(state.cards).toHaveLength(1);
    expect(state.cards[0]).toMatchObject({ id: "insight", generating: false, failed: false });
  });
});

describe("failure", () => {
  const good = {
    op: "add",
    path: "/root",
    value: "root",
  };

  it("restores the previous Card when a generation applies nothing", () => {
    const state = reduceCards(emptyCardsState(), [
      { kind: "card_begin", card_id: "result" },
      { kind: "card_op", card_id: "result", op: line(good) },
      { kind: "card_end", card_id: "result", ok: true },
      // A second generation that never lands a usable line.
      { kind: "card_begin", card_id: "result" },
      { kind: "card_op", card_id: "result", op: '{"op":"add","path":\n' },
      { kind: "card_end", card_id: "result", ok: false },
    ]);
    const result = card(state, "result");
    expect(result.spec.root).toBe("root");
    expect(result.failed).toBe(true);
    expect(result.generating).toBe(false);
    expect(hasFailure(state)).toBe(true);
  });

  it("keeps the partial Card when the generation did apply something", () => {
    const state = reduceCards(emptyCardsState(), [
      { kind: "card_begin", card_id: "result" },
      { kind: "card_op", card_id: "result", op: line(good) },
      { kind: "card_end", card_id: "result", ok: false },
    ]);
    expect(card(state, "result").spec.root).toBe("root");
    expect(card(state, "result").failed).toBe(true);
  });

  it("clears the failure when the next generation begins", () => {
    const state = reduceCards(emptyCardsState(), [
      { kind: "card_begin", card_id: "result" },
      { kind: "card_end", card_id: "result", ok: false },
      { kind: "card_begin", card_id: "result" },
    ]);
    expect(card(state, "result").failed).toBe(false);
    expect(hasFailure(state)).toBe(false);
  });
});

describe("kind label", () => {
  it("names the known kinds and falls back to the raw id", () => {
    expect(cardKindLabel("task-board")).toBe("Task board");
    expect(cardKindLabel("result")).toBe("Result");
    expect(cardKindLabel("insight")).toBe("Insight");
    expect(cardKindLabel("other")).toBe("other");
  });
});
