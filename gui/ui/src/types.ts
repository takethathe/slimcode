// The GUI wire event contract: the flat `{kind: ...}` JSON the Tauri host
// pushes over the per-prompt Channel (mirrors the Rust `GuiWireEvent`).
// The frontend never sees an `SdkEvent`.

export type StopReason = "completed" | "cancelled";

export type TokenUsage = {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  prompt_tokens_details: {
    cached_tokens: number;
    cache_creation_input_tokens: number;
  } | null;
};

export type ToolCall = {
  id: string;
  name: string;
  /** Raw JSON arguments string as the model emitted it. */
  arguments: string;
};

export type Part = { type: "text"; text: string };

export type AgentMessage =
  | {
      kind: "llm";
      role: "user" | "assistant" | "tool" | "system";
      parts: Part[];
      tool_calls?: ToolCall[];
      tool_call_id?: string;
    }
  | {
      kind: "compact_summary";
      summary: string;
      tokens_before: number;
      previous_summary?: string;
    };

export type GuiWireEvent =
  | { kind: "turn"; turn: number }
  | { kind: "reasoning"; text: string }
  | { kind: "text"; text: string }
  | {
      kind: "tool_start";
      tool_call_id: string;
      name: string;
      arguments: string;
    }
  | {
      kind: "tool_result";
      tool_call_id: string;
      name: string;
      ok: boolean;
      result: string;
    }
  | { kind: "stop"; reason: StopReason }
  | { kind: "usage"; usage: TokenUsage }
  | { kind: "message"; message: AgentMessage; usage: TokenUsage | null }
  | { kind: "turn_finished"; stop_reason: StopReason }
  | { kind: "compacted"; before: number; after: number }
  | { kind: "error"; message: string };

export type ContextInfo = {
  percent: number;
  window: number;
};

export type SessionInfo = {
  id: string;
  title: string | null;
  /** Unix seconds of the log's last modification. */
  modified: number;
  messages: number;
};

export type SessionSnapshot = {
  id: string;
  title: string | null;
  model: string;
  context: ContextInfo;
  messages: AgentMessage[];
  usage: TokenUsage;
  warning: string | null;
};

export type GuiSettings = {
  model: string | null;
  base_url: string | null;
  cache: boolean | null;
  api_key: string | null;
};
