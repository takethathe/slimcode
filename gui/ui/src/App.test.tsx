// @vitest-environment jsdom
// Wiring assertions for the resident Usage HUD in <App>: which host event
// recalibrates the cumulative total from a snapshot, and that a snapshot which
// lands after a newer local delta never rolls the total back (spec
// .scratch/gui-usage-hud, ADR-0029 D5). Events are pushed through the very
// callback the host streams them on, so the queue/flush path is exercised.
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "./App";
import type { GuiWireEvent, SessionSnapshot, TokenUsage } from "./types";

// The host seam is stubbed wholesale: these tests hand-drive the wire events,
// so nothing here needs a Tauri runtime.
const host = vi.hoisted(() => ({
  newSession: vi.fn(),
  resumeSession: vi.fn(),
  deleteSession: vi.fn(),
  listSessions: vi.fn(),
  getSession: vi.fn(),
  complete: vi.fn(),
  cancel: vi.fn(),
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  prompt: vi.fn(),
  compactNow: vi.fn(),
}));

vi.mock("./api", () => ({
  api: host,
  errorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ close: () => Promise.resolve() }),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

function usage(prompt: number, completion: number, cached = 0): TokenUsage {
  return {
    prompt_tokens: prompt,
    completion_tokens: completion,
    total_tokens: prompt + completion,
    prompt_tokens_details: {
      cached_tokens: cached,
      cache_creation_input_tokens: 0,
    },
  };
}

function snapshot(value: TokenUsage, percent = 0): SessionSnapshot {
  return {
    id: "s1",
    title: null,
    model: "gpt",
    context: { percent, window: 200_000 },
    messages: [],
    usage: value,
    warning: null,
  };
}

// The queue callback the host streams events on; captured from the Compact
// button, which is the shortest path to `api.compactNow(onEvent)`.
let emit: ((event: GuiWireEvent) => void) | null = null;

beforeEach(() => {
  emit = null;
  for (const fn of Object.values(host)) fn.mockReset();
  host.getSession.mockResolvedValue(snapshot(usage(0, 0)));
  host.listSessions.mockResolvedValue([]);
  host.complete.mockResolvedValue([]);
  host.compactNow.mockImplementation(
    (onEvent: (event: GuiWireEvent) => void) => {
      emit = onEvent;
      return Promise.resolve();
    },
  );
  // jsdom has no frame clock; run the flush inline so each batch lands inside
  // the `act` that emitted it.
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    cb(0);
    return 0;
  });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

async function mountApp(): Promise<void> {
  render(<App />);
  await waitFor(() => expect(host.getSession).toHaveBeenCalledTimes(1));
  fireEvent.click(screen.getByRole("button", { name: "Compact" }));
  await waitFor(() => expect(emit).not.toBeNull());
}

describe("Usage HUD recalibration", () => {
  it("refreshes the snapshot when a batch carries `compacted`", async () => {
    await mountApp();
    host.getSession.mockResolvedValue(snapshot(usage(1_500, 200), 4.5));

    act(() => emit?.({ kind: "compacted", before: 1_500, after: 300 }));

    await waitFor(() => expect(screen.getByText("↑1.5k")).toBeTruthy());
    expect(screen.getByText("↓200")).toBeTruthy();
    expect(screen.getByText("4.5%/200k")).toBeTruthy();
  });

  it("keeps a newer local total when an older snapshot lands late", async () => {
    await mountApp();

    // The `message` batch asks for a snapshot; hold the answer back so the
    // following `usage` event is folded into the total first.
    let answer: ((value: SessionSnapshot) => void) | null = null;
    host.getSession.mockImplementationOnce(
      () =>
        new Promise<SessionSnapshot>((resolve) => {
          answer = resolve;
        }),
    );
    act(() =>
      emit?.({
        kind: "message",
        message: { kind: "llm", role: "assistant", parts: [] },
        usage: null,
      }),
    );
    await waitFor(() => expect(host.getSession).toHaveBeenCalledTimes(2));

    act(() => emit?.({ kind: "usage", usage: usage(120, 12) }));
    expect(screen.getByText("↑120")).toBeTruthy();

    // The stale snapshot predates the delta and must be ignored.
    await act(async () => {
      answer?.(snapshot(usage(0, 0)));
    });
    expect(screen.getByText("↑120")).toBeTruthy();
    expect(screen.getByText("↓12")).toBeTruthy();
  });
});
