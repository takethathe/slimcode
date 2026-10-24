import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  CompletionItem,
  GuiSettings,
  GuiWireEvent,
  SessionInfo,
  SessionSnapshot,
} from "./types";
import type { CardEvent, CardSnapshot } from "./cards";

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export const api = {
  newSession: (): Promise<void> => invoke("new_session"),
  resumeSession: (id: string): Promise<void> => invoke("resume_session", { id }),
  deleteSession: (id: string): Promise<void> => invoke("delete_session", { id }),
  listSessions: (): Promise<SessionInfo[]> => invoke("list_sessions"),
  getSession: (): Promise<SessionSnapshot> => invoke("get_session"),
  /** Ranked `/` completion candidates for the leading token (ADR-0026). */
  complete: (input: string): Promise<CompletionItem[]> =>
    invoke("complete", { input }),
  cancel: (): Promise<void> => invoke("cancel"),
  getSettings: (): Promise<GuiSettings> => invoke("get_settings"),
  setSettings: (settings: GuiSettings): Promise<void> =>
    invoke("set_settings", { settings }),

  /** Run one turn, streaming wire events to `onEvent`. */
  prompt(text: string, onEvent: (event: GuiWireEvent) => void): Promise<void> {
    const channel = new Channel<GuiWireEvent>();
    channel.onmessage = onEvent;
    return invoke("prompt", { text, channel }).then(() => undefined);
  },

  /** Force an on-demand compaction, streaming wire events to `onEvent`. */
  compactNow(onEvent: (event: GuiWireEvent) => void): Promise<void> {
    const channel = new Channel<GuiWireEvent>();
    channel.onmessage = onEvent;
    return invoke("compact_now", { channel }).then(() => undefined);
  },

  /**
   * Subscribe to the Card channel once (ADR-0030 D4): the resolved value is
   * the current full snapshot, and every later change arrives on `onEvent` as
   * one of the four flat Card events. `systemPrompt` is the catalog-generated
   * description the host uses for automatic generations too.
   */
  subscribeCards(
    onEvent: (event: CardEvent) => void,
    systemPrompt: string,
  ): Promise<CardSnapshot[]> {
    const channel = new Channel<CardEvent>();
    channel.onmessage = onEvent;
    return invoke("subscribe_cards", { channel, systemPrompt });
  },

  /** Ask the host for one Card generation (the retry / refresh path). */
  regenerateCards(systemPrompt: string): Promise<void> {
    return invoke("regenerate_cards", { systemPrompt });
  },
};

export { errorMessage };
