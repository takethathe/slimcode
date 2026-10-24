// The Card view's component vocabulary (ADR-0030 D6/D7): zod prop schemas
// plus the json-render catalog they define. This module is the single
// description of what a Card may contain — the system prompt is generated
// from it at runtime (`cardSystemPrompt`), so there is no second, build-time
// copy that could drift.
//
// Ten components, hand-written in the GUI's slate/indigo language (D6): a
// read-only surface does not need the shadcn/Radix package.

import { defineCatalog } from "@json-render/core";
import { schema } from "@json-render/react/schema";
import { z } from "zod";

/** The fixed Card kind vocabulary (ADR-0030 D2). A kind is a Card's identity. */
export const CARD_KINDS = ["task-board", "result", "insight"] as const;
export type CardKind = (typeof CARD_KINDS)[number];

export function isCardKind(value: unknown): value is CardKind {
  return (
    typeof value === "string" && (CARD_KINDS as readonly string[]).includes(value)
  );
}

const point = z.enum(["sm", "md", "lg"]).optional();

export const catalog = defineCatalog(schema, {
  components: {
    Card: {
      props: z.object({
        title: z.string(),
        subtitle: z.string().optional(),
      }),
      description:
        "The root container of a Card: a title, an optional subtitle, and the sections below it. Use exactly one per Card.",
      slots: ["default"],
    },
    Stack: {
      props: z.object({
        direction: z.enum(["vertical", "horizontal"]).optional(),
        gap: point,
      }),
      description:
        "Groups children in a vertical (default) or horizontal stack. Use it for rows of badges or side-by-side key/value pairs.",
      slots: ["default"],
    },
    Text: {
      props: z.object({
        content: z.string(),
        weight: z.enum(["normal", "muted", "strong"]).optional(),
        size: point,
      }),
      description:
        "One short paragraph of plain text. Keep a single Text under about 120 characters; use several Text elements instead of one long blob.",
    },
    List: {
      props: z.object({
        items: z.array(
          z.object({
            text: z.string(),
            done: z.boolean().optional(),
          }),
        ),
      }),
      description:
        "An ordered list of short entries. `done` marks an entry complete (task boards, checklists). Emit one entry per patch where possible.",
    },
    KeyValue: {
      props: z.object({
        label: z.string(),
        value: z.string(),
      }),
      description: "One label/value pair, e.g. branch or duration.",
    },
    Badge: {
      props: z.object({
        text: z.string(),
        tone: z.enum(["neutral", "info", "success", "warning", "danger"]).optional(),
      }),
      description: "A small status pill for a short status word.",
    },
    Progress: {
      props: z.object({
        value: z.number(),
        label: z.string().optional(),
      }),
      description: "A 0-100 progress bar with an optional label.",
    },
    Divider: {
      props: z.object({}),
      description: "A thin horizontal rule separating sections.",
    },
    Markdown: {
      props: z.object({
        content: z.string(),
      }),
      description:
        "A short markdown block (headings, links, code). Renders with the same rules as the transcript. Use it only for prose that needs formatting.",
    },
  },
  // A Card is read-only: it renders state and never dispatches an interaction.
  actions: {},
});

/**
 * The system prompt for one generation, generated from the catalog at runtime
 * (ADR-0030 D6). The host never stores a component description of its own.
 *
 * The extra rules are the Card-specific contract:
 * - the first line declares the Card kind (the host needs the target card
 *   before it can stream ops into it);
 * - element-level granularity so the Card grows visibly (D7);
 * - no interaction: a Card is a derived, read-only view.
 */
export function cardSystemPrompt(): string {
  return catalog.prompt({
    system:
      "You maintain the Card view of a coding-agent GUI: a small, read-only panel that shows the current state of the session. You never edit code and you are not the agent doing the work.",
    customRules: [
      `Your first output line MUST be {"op":"add","path":"/kind","value":"<kind>"} where <kind> is one of: ${CARD_KINDS.join(
        ", ",
      )}. The kind is the Card's identity: reusing an existing kind replaces that Card, so keep a task board on the task-board kind across turns.`,
      "Use task-board for ongoing work and its progress, result for what the last turn produced, insight for metrics or observations.",
      "After the kind line, emit one patch per line. The Card is rebuilt from an empty spec on every generation: start with /root and the root Card element, then reproduce the parts of the existing spec that still apply (copy them verbatim) and add or change only what this turn changed. Never patch an element you have not created in this stream.",
      "Use exactly one Card element as the root: the first structural op after the kind line is {\"op\":\"add\",\"path\":\"/root\",\"value\":\"root\"} followed by {\"op\":\"add\",\"path\":\"/elements/root\",\"value\":{\"type\":\"Card\",\"props\":{\"title\":\"...\"},\"children\":[...]}}.",
      "Add one element per patch, in the order the reader should see it, and never wrap the whole Card in a single add value.",
      "Keep every Text content under about 120 characters; split longer content into several Text elements instead of one long paragraph.",
      "Prefer structure (List / KeyValue / Badge / Progress) over prose. Do not repeat the conversation, summarise the state a reader would want at a glance.",
      "Output only JSONL patches: no prose, no markdown fences, no blank lines.",
    ],
  });
}

/** The static demo used by the debug entry and the frontend tests. */
export const CARD_DEMO_OPS: string[] = [
  '{"op":"add","path":"/kind","value":"result"}',
  '{"op":"add","path":"/root","value":"root"}',
  '{"op":"add","path":"/elements/root","value":{"type":"Card","props":{"title":"Card view","subtitle":"live preview"},"children":["status","summary","progress","rows","done"]}}',
  '{"op":"add","path":"/elements/status","value":{"type":"Badge","props":{"text":"growing","tone":"info"},"children":[]}}',
  '{"op":"add","path":"/elements/summary","value":{"type":"Text","props":{"content":"The panel paints one patch at a time, so the card grows visibly."},"children":[]}}',
  '{"op":"add","path":"/elements/progress","value":{"type":"Progress","props":{"value":45,"label":"rendering"},"children":[]}}',
  '{"op":"add","path":"/elements/rows","value":{"type":"List","props":{"items":[{"text":"root painted","done":true},{"text":"elements arriving","done":true},{"text":"wrap up"}]},"children":[]}}',
  '{"op":"add","path":"/elements/done","value":{"type":"Text","props":{"content":"Done.","weight":"muted"},"children":[]}}',
];
