// The transcript block model: the frontend's rendering state machine.
//
// Wire events (streaming) and the session snapshot (resume) both reduce into
// a list of `Block`s. The rules (spec §transcript-rendering-rules):
//   - `text` fragments append to the current assistant block;
//   - `reasoning` fragments append to the current assistant block's reasoning
//     (rendered dim and collapsible);
//   - `tool_start` / `tool_result` pair into a card (running / ok / error),
//     unpaired starts hang as running;
//   - `usage` renders a token-usage line;
//   - `compacted` renders a collapsible notice;
//   - a user `message` opens a user block; an assistant `message` closes the
//     current assistant block.
// Skill / command emphasis is derived from message text (ADR-0027), not the
// wire: a user message starting with a `<skill>` block becomes a user block
// carrying a skill Transcript chip (its XML hidden), a leading `/{token}`
// carries a command chip, and a `read` whose result starts with `<skill>` is
// replaced in place by a Skill active block. Live and resume share these rules.

import type { Chip } from "./chip";
import type {
  AgentMessage,
  GuiWireEvent,
  TokenUsage,
} from "./types";

export type ToolStatus = "running" | "ok" | "error";

// The chip types live in the shared chip module (used by both the transcript
// and the input box); re-exported here so the block model reads self-contained.
export type { Chip, ChipCategory } from "./chip";

export type Block =
  | { id: string; kind: "user"; text: string; chip: Chip | null }
  | {
      id: string;
      kind: "assistant";
      text: string;
      reasoning: string;
      complete: boolean;
    }
  | {
      id: string;
      kind: "tool";
      callId: string;
      name: string;
      arguments: string;
      status: ToolStatus;
      result: string;
    }
  // Skill active block: shown in place of the `read` tool card when the model
  // activates a skill by reading its `SKILL.md` (the read result is a `<skill>`
  // block). The user-trigger path instead surfaces a skill chip in a user block.
  | { id: string; kind: "skill"; name: string }
  | { id: string; kind: "usage"; usage: TokenUsage }
  | { id: string; kind: "compacted"; before: number; after: number | null }
  | { id: string; kind: "error"; text: string };

let blockSeq = 0;
function nextId(): string {
  blockSeq += 1;
  return `b${blockSeq}`;
}

export function newBlockSeq(): void {
  blockSeq = 0;
}

/** A standalone token-usage block, appended on demand for the `/usage`
 *  command (mirrors the TUI's `show_usage`). Carries the session's cumulative
 *  usage, read from the current snapshot. */
export function usageNotice(usage: TokenUsage): Block {
  return { id: nextId(), kind: "usage", usage };
}

function reduceBlock(state: Block[], event: GuiWireEvent): Block[] {
  const blocks = [...state];
  switch (event.kind) {
    case "message": {
      const message = event.message;
      if (message.kind !== "llm") break;
      if (message.role === "user") {
        blocks.push(userBlock(nextId(), textOf(message)));
      } else if (message.role === "assistant") {
        // Close the in-progress assistant block (if any).
        const last = blocks[blocks.length - 1];
        if (last && last.kind === "assistant" && !last.complete) {
          blocks[blocks.length - 1] = { ...last, complete: true };
        }
      }
      break;
    }
    case "reasoning": {
      const last = blocks[blocks.length - 1];
      if (last && last.kind === "assistant") {
        blocks[blocks.length - 1] = {
          ...last,
          reasoning: last.reasoning + event.text,
        };
      } else {
        blocks.push({
          id: nextId(),
          kind: "assistant",
          text: "",
          reasoning: event.text,
          complete: false,
        });
      }
      break;
    }
    case "text": {
      const last = blocks[blocks.length - 1];
      if (last && last.kind === "assistant") {
        blocks[blocks.length - 1] = {
          ...last,
          text: last.text + event.text,
        };
      } else {
        blocks.push({
          id: nextId(),
          kind: "assistant",
          text: event.text,
          reasoning: "",
          complete: false,
        });
      }
      break;
    }
    case "tool_start": {
      blocks.push({
        id: nextId(),
        kind: "tool",
        callId: event.tool_call_id,
        name: event.name,
        arguments: event.arguments,
        status: "running",
        result: "",
      });
      break;
    }
    case "tool_result": {
      const index = blocks.findIndex(
        (b) => b.kind === "tool" && b.callId === event.tool_call_id,
      );
      if (index >= 0) {
        const tool = blocks[index] as Extract<Block, { kind: "tool" }>;
        // Only a `read` is a skill-activation candidate (mirrors the CLI
        // renderer, which suppresses the card only for a skill-serving read).
        const skill = tool.name === "read" ? parseSkillBlock(event.result) : null;
        if (skill) {
          // A skill-serving `read` becomes a Skill active block in place, so
          // the activation is not shown twice (spec: asymmetric rendering).
          blocks[index] = { id: tool.id, kind: "skill", name: skill.name };
        } else {
          blocks[index] = {
            ...tool,
            status: event.ok ? "ok" : "error",
            result: event.result,
          };
        }
      }
      break;
    }
    case "stop":
      // The display stream for this turn is done; close the assistant block.
      {
        const last = blocks[blocks.length - 1];
        if (last && last.kind === "assistant" && !last.complete) {
          blocks[blocks.length - 1] = { ...last, complete: true };
        }
      }
      break;
    case "usage": {
      blocks.push({ id: nextId(), kind: "usage", usage: event.usage });
      break;
    }
    case "compacted": {
      blocks.push({
        id: nextId(),
        kind: "compacted",
        before: event.before,
        after: event.after,
      });
      break;
    }
    case "error": {
      blocks.push({ id: nextId(), kind: "error", text: event.message });
      break;
    }
    case "turn":
    case "turn_finished":
      break;
  }
  return blocks;
}

/** Reduce a wire event stream into transcript blocks. */
export function reduceTranscript(
  state: Block[],
  events: GuiWireEvent[],
): Block[] {
  let blocks = state;
  for (const event of events) {
    blocks = reduceBlock(blocks, event);
  }
  return blocks;
}

function textOf(message: Extract<AgentMessage, { kind: "llm" }>): string {
  return message.parts.map((part) => part.text).join("");
}

/** Parse a `<skill>` activation block. Mirrors `slimcode_app::skills::parse_skill_block`:
 *  only when `text` *starts with* `<skill name="…"` does it yield the skill's
 *  `name` and inner `content`; anything else (a mid-text mention, an
 *  unterminated tag, plain prose) yields `null`. A trailing task argument after
 *  `</skill>` stays outside the returned content. */
export function parseSkillBlock(text: string): { name: string; content: string } | null {
  const rest = stripPrefix(text, '<skill name="');
  if (rest === null) return null;
  const atQuote = rest.indexOf('" location="');
  if (atQuote < 0) return null;
  const name = rest.slice(0, atQuote);
  const afterLoc = stripPrefix(rest.slice(atQuote), '" location="');
  if (afterLoc === null) return null;
  const atTag = afterLoc.indexOf('">\n');
  if (atTag < 0) return null;
  const body = afterLoc.slice(atTag + '">\n'.length);
  const atClose = body.indexOf("\n</skill>");
  if (atClose < 0) return null;
  return { name, content: body.slice(0, atClose) };
}

function stripPrefix(text: string, prefix: string): string | null {
  return text.startsWith(prefix) ? text.slice(prefix.length) : null;
}

/** Extract the leading command token (`/{token}`) from user text, if present.
 *  A bare `/` or `/` followed by whitespace is not a token. Returns the token
 *  (slash included) and the remaining body. */
function leadingCommandToken(
  text: string,
): { token: string; rest: string } | null {
  const trimmed = text.replace(/^\s+/, "");
  if (!trimmed.startsWith("/")) return null;
  const after = trimmed.slice(1);
  if (after.length === 0 || /^\s/.test(after)) return null;
  const space = after.search(/\s/);
  if (space < 0) return { token: `/${after}`, rest: "" };
  return {
    token: `/${after.slice(0, space)}`,
    rest: after.slice(space).replace(/^\s+/, ""),
  };
}

/** Build a user block from raw message text: detect a skill trigger first, then
 *  a command token, else plain text. Skill triggers hide the XML and show a
 *  `/{name}` chip plus any trailing task argument. */
function userBlock(id: string, text: string): Block {
  const skill = parseSkillBlock(text);
  if (skill) {
    // Slice the trailing task argument at the same boundary the parser uses
    // (`\n</skill>`), so an inline `</skill>` inside the skill body never leaks
    // the XML tail into the block.
    const close = text.indexOf("\n</skill>");
    const arg = close < 0 ? "" : text.slice(close + "\n</skill>".length).trim();
    return {
      id,
      kind: "user",
      chip: { category: "skill", token: `/${skill.name}` },
      text: arg,
    };
  }
  const command = leadingCommandToken(text);
  if (command) {
    return {
      id,
      kind: "user",
      chip: { category: "command", token: command.token },
      text: command.rest,
    };
  }
  return { id, kind: "user", chip: null, text };
}

/** Build the transcript from a resumed session's history. */
export function blocksFromHistory(messages: AgentMessage[]): Block[] {
  const blocks: Block[] = [];
  // Tool results arrive as separate `tool` messages; remember the assistant
  // tool cards awaiting their result.
  const pending = new Map<string, number>();
  for (const message of messages) {
    if (message.kind === "compact_summary") {
      blocks.push({
        id: nextId(),
        kind: "compacted",
        before: message.tokens_before,
        // The replayed log has no "after" figure (only the live event
        // carries it); leave it unknown.
        after: null,
      });
      continue;
    }
    if (message.role === "user") {
      blocks.push(userBlock(nextId(), textOf(message)));
      continue;
    }
    if (message.role === "assistant") {
      const text = textOf(message);
      const calls = message.tool_calls ?? [];
      if (text) {
        blocks.push({
          id: nextId(),
          kind: "assistant",
          text,
          reasoning: "",
          complete: true,
        });
      }
      for (const call of calls) {
        const index = blocks.length;
        blocks.push({
          id: nextId(),
          kind: "tool",
          callId: call.id,
          name: call.name,
          arguments: call.arguments,
          status: "running",
          result: "",
        });
        pending.set(call.id, index);
      }
      continue;
    }
    if (message.role === "tool") {
      const callId = message.tool_call_id;
      if (callId) {
        const index = pending.get(callId);
        if (index !== undefined && blocks[index] && blocks[index].kind === "tool") {
          const tool = blocks[index] as Extract<Block, { kind: "tool" }>;
          const result = textOf(message);
          const skill = tool.name === "read" ? parseSkillBlock(result) : null;
          if (skill) {
            // Same rule as the live stream: a skill-serving read is replaced
            // in place by a Skill active block.
            blocks[index] = { id: tool.id, kind: "skill", name: skill.name };
          } else {
            blocks[index] = {
              ...tool,
              status: "ok",
              result,
            };
          }
          pending.delete(callId);
        }
      }
    }
  }
  return blocks;
}
