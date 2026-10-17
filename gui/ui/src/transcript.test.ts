import { describe, expect, it, beforeEach } from "vitest";
import {
  blocksFromHistory,
  newBlockSeq,
  parseSkillBlock,
  reduceTranscript,
  type Block,
} from "./transcript";
import { INTERCEPTED_COMMANDS } from "./commands";
import type { AgentMessage, GuiWireEvent } from "./types";

// A `<skill>` block as the backend injects it for a user trigger, with an
// optional trailing task argument after `</skill>`.
function skillBlock(name: string, body = "Body.", arg?: string): string {
  const block = `<skill name="${name}" location="/x/${name}/SKILL.md">\nReferences are relative to /x/${name}.\n\n${body}\n</skill>`;
  return arg ? `${block}\n\n${arg}` : block;
}

function userEvent(text: string): GuiWireEvent {
  const message: AgentMessage = {
    kind: "llm",
    role: "user",
    parts: [{ type: "text", text }],
  };
  return { kind: "message", message, usage: null };
}

// Narrow the first block to a user block for assertions.
function user(block: Block | undefined): Extract<Block, { kind: "user" }> {
  if (!block || block.kind !== "user") {
    throw new Error(`expected a user block, got ${JSON.stringify(block)}`);
  }
  return block;
}

beforeEach(() => {
  newBlockSeq();
});

describe("parseSkillBlock", () => {
  it("splits name and inner content only when the text starts with the block", () => {
    expect(parseSkillBlock(skillBlock("grill"))).toEqual({
      name: "grill",
      content:
        "References are relative to /x/grill.\n\nBody.",
    });
  });

  it("keeps a trailing task argument outside the content", () => {
    const parsed = parseSkillBlock(skillBlock("grill", "Body.", "my plan"));
    expect(parsed?.name).toBe("grill");
    expect(parsed?.content).not.toContain("my plan");
  });

  it("returns null for non-block, unterminated, and mid-text mentions", () => {
    expect(parseSkillBlock("hello")).toBeNull();
    expect(
      parseSkillBlock('<skill name="x" location="/y">\nunterminated'),
    ).toBeNull();
    expect(
      parseSkillBlock('see <skill name="x" location="/y">\nz\n</skill>'),
    ).toBeNull();
  });
});

describe("user block: skill trigger (ticket 02)", () => {
  it("rebuilds a `<skill>` user message into a skill chip plus task arg", () => {
    const [block] = reduceTranscript([], [userEvent(skillBlock("grill", "Body.", "my plan"))]);
    expect(user(block).chip).toEqual({ category: "skill", token: "/grill" });
    expect(user(block).text).toBe("my plan");
    expect(user(block).text).not.toContain("<skill");
  });

  it("still emits a chip-only user block when there is no task argument", () => {
    const [block] = reduceTranscript([], [userEvent(skillBlock("grill"))]);
    expect(user(block).chip).toEqual({ category: "skill", token: "/grill" });
    expect(user(block).text).toBe("");
  });

  it("does not treat a mid-text `<skill>` mention as a trigger", () => {
    const text = `here is an example:\n${skillBlock("grill")}`;
    const [block] = reduceTranscript([], [userEvent(text)]);
    expect(user(block).chip).toBeNull();
    expect(user(block).text).toBe(text);
  });

  it("splits the task argument at the block's real end, not an inline </skill>", () => {
    // A skill body that documents the XML form can contain `</skill>` inline;
    // the trailing task argument must not be sliced from there (no XML leak).
    const text = skillBlock(
      "grill",
      "an inline </skill> inside the body",
      "my task arg",
    );
    const [block] = reduceTranscript([], [userEvent(text)]);
    expect(user(block).chip).toEqual({ category: "skill", token: "/grill" });
    expect(user(block).text).toBe("my task arg");
    expect(user(block).text).not.toContain("</skill>");
  });
});

describe("user block: command chip (ticket 03)", () => {
  it("chips a leading `/{token}` and keeps the rest as body", () => {
    const [block] = reduceTranscript([], [userEvent("/deploy prod us")]);
    expect(user(block).chip).toEqual({ category: "command", token: "/deploy" });
    expect(user(block).text).toBe("prod us");
  });

  it("leaves plain prompt text without a chip", () => {
    const [block] = reduceTranscript([], [userEvent("review this diff")]);
    expect(user(block).chip).toBeNull();
    expect(user(block).text).toBe("review this diff");
  });

  it("prefers the skill chip over the command chip", () => {
    // A recognised skill trigger starts with `<skill>` (already expanded), so
    // the skill branch wins even though a `/` token would otherwise match.
    const [block] = reduceTranscript([], [userEvent(skillBlock("grill", "Body.", "x"))]);
    expect(user(block).chip?.category).toBe("skill");
  });

  it("does not chip a bare slash", () => {
    const [block] = reduceTranscript([], [userEvent("/ hello")]);
    expect(user(block).chip).toBeNull();
  });
});

describe("model read activation (ticket 04)", () => {
  const readStart: GuiWireEvent = {
    kind: "tool_start",
    tool_call_id: "c1",
    name: "read",
    arguments: '{"path":"/x/grill/SKILL.md"}',
  };

  it("replaces a skill-serving read card with a skill block in place", () => {
    const blocks = reduceTranscript([], [
      readStart,
      {
        kind: "tool_result",
        tool_call_id: "c1",
        name: "read",
        ok: true,
        result: skillBlock("grill"),
      },
    ]);
    expect(blocks.some((b) => b.kind === "tool")).toBe(false);
    expect(blocks[0]).toMatchObject({ kind: "skill", name: "grill" });
  });

  it("keeps an ordinary read result as a tool card", () => {
    const blocks = reduceTranscript([], [
      readStart,
      {
        kind: "tool_result",
        tool_call_id: "c1",
        name: "read",
        ok: true,
        result: "file contents here",
      },
    ]);
    expect(blocks[0]).toMatchObject({ kind: "tool", status: "ok" });
  });

  it("does not suppress a non-read tool whose result starts with <skill>", () => {
    // Only a skill-serving `read` is a candidate; a bash that echoes a skill
    // block keeps its card (and its status), mirroring the CLI renderer.
    const blocks = reduceTranscript([], [
      { kind: "tool_start", tool_call_id: "c1", name: "bash", arguments: "{}" },
      {
        kind: "tool_result",
        tool_call_id: "c1",
        name: "bash",
        ok: true,
        result: skillBlock("grill"),
      },
    ]);
    expect(blocks.some((b) => b.kind === "tool")).toBe(true);
    expect(blocks.some((b) => b.kind === "skill")).toBe(false);
  });
});

describe("intercepted local commands never chip (ticket 03 guard)", () => {
  it("the frontend intercepts the effect-mapped commands so they never reach the reducer", () => {
    for (const command of ["/new", "/compact", "/usage", "/help", "/exit"]) {
      expect(INTERCEPTED_COMMANDS.has(command)).toBe(true);
    }
  });
});

describe("resume replay parity (ticket 05)", () => {
  it("replays a skill user message and a skill-serving read like the live stream", () => {
    const messages: AgentMessage[] = [
      { kind: "llm", role: "user", parts: [{ type: "text", text: skillBlock("grill", "Body.", "plan") }] },
      {
        kind: "llm",
        role: "assistant",
        parts: [{ type: "text", text: "" }],
        tool_calls: [{ id: "c1", name: "read", arguments: '{"path":"/x/grill/SKILL.md"}' }],
      },
      { kind: "llm", role: "tool", tool_call_id: "c1", parts: [{ type: "text", text: skillBlock("grill") }] },
    ];
    const replayed = blocksFromHistory(messages);

    const live = reduceTranscript([], [
      userEvent(skillBlock("grill", "Body.", "plan")),
      {
        kind: "tool_start",
        tool_call_id: "c1",
        name: "read",
        arguments: '{"path":"/x/grill/SKILL.md"}',
      },
      {
        kind: "tool_result",
        tool_call_id: "c1",
        name: "read",
        ok: true,
        result: skillBlock("grill"),
      },
    ]);

    const shape = (b: Block) =>
      b.kind === "user"
        ? { kind: "user", chip: b.chip, text: b.text }
        : { kind: b.kind, ...(b.kind === "skill" ? { name: b.name } : {}) };

    expect(replayed.map(shape)).toEqual(live.map(shape));
    expect(replayed.some((b) => b.kind === "tool")).toBe(false);
    expect(replayed.some((b) => b.kind === "skill")).toBe(true);
  });

  it("replays an ordinary read result as a tool card", () => {
    const messages: AgentMessage[] = [
      {
        kind: "llm",
        role: "assistant",
        parts: [{ type: "text", text: "" }],
        tool_calls: [{ id: "c1", name: "read", arguments: "{}" }],
      },
      { kind: "llm", role: "tool", tool_call_id: "c1", parts: [{ type: "text", text: "plain file" }] },
    ];
    const blocks = blocksFromHistory(messages);
    expect(blocks.find((b) => b.kind === "tool")).toMatchObject({ kind: "tool", status: "ok" });
  });
});
