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

import type {
  AgentMessage,
  GuiWireEvent,
  TokenUsage,
} from "./types";

export type ToolStatus = "running" | "ok" | "error";

export type Block =
  | { id: string; kind: "user"; text: string }
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

function reduceBlock(state: Block[], event: GuiWireEvent): Block[] {
  const blocks = [...state];
  switch (event.kind) {
    case "message": {
      const message = event.message;
      if (message.kind !== "llm") break;
      if (message.role === "user") {
        blocks.push({ id: nextId(), kind: "user", text: textOf(message) });
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
        blocks[index] = {
          ...(blocks[index] as Extract<Block, { kind: "tool" }>),
          status: event.ok ? "ok" : "error",
          result: event.result,
        };
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
      blocks.push({ id: nextId(), kind: "user", text: textOf(message) });
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
          blocks[index] = {
            ...(blocks[index] as Extract<Block, { kind: "tool" }>),
            status: "ok",
            result: textOf(message),
          };
          pending.delete(callId);
        }
      }
    }
  }
  return blocks;
}
