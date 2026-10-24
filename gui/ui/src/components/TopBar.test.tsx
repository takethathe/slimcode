// @vitest-environment jsdom
// DOM assertions for the Usage HUD seam: what a reader can see in the top bar
// (spec .scratch/gui-usage-hud). Tests assert the badge text and the hover
// tooltip string — never Tailwind classes or the component tree.
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { TopBar } from "./TopBar";
import { usageTooltip } from "../usage";
import type { ContextInfo, TokenUsage } from "../types";

afterEach(cleanup);

const ctx: ContextInfo = { percent: 12.3, window: 200_000 };

function usage(
  prompt: number,
  completion: number,
  cached: number,
  cacheCreation: number,
): TokenUsage {
  return {
    prompt_tokens: prompt,
    completion_tokens: completion,
    total_tokens: prompt + completion,
    prompt_tokens_details: {
      cached_tokens: cached,
      cache_creation_input_tokens: cacheCreation,
    },
  };
}

const zero: TokenUsage = {
  prompt_tokens: 0,
  completion_tokens: 0,
  total_tokens: 0,
  prompt_tokens_details: null,
};

describe("TopBar Usage HUD", () => {
  it("shows all four segments in the empty state", () => {
    render(
      <TopBar
        title={null}
        model="gpt"
        context={{ percent: 0, window: 200_000 }}
        onCompact={() => {}}
        compacting={false}
        pulse={0}
        usage={zero}
        latest={null}
      />,
    );
    expect(screen.getByText("↑0")).toBeTruthy();
    expect(screen.getByText("↓0")).toBeTruthy();
    expect(screen.getByText("CH—")).toBeTruthy();
    expect(screen.getByText("0%/200k")).toBeTruthy();
  });

  it("renders the four live numbers after a cached request", () => {
    render(
      <TopBar
        title="Session"
        model="gpt"
        context={ctx}
        onCompact={() => {}}
        compacting={false}
        pulse={0}
        usage={usage(3396, 101, 3334, 0)}
        latest={usage(3396, 101, 3334, 0)}
      />,
    );
    expect(screen.getByText("↑3.4k")).toBeTruthy();
    expect(screen.getByText("↓101")).toBeTruthy();
    expect(screen.getByText("CH98.2%")).toBeTruthy();
    expect(screen.getByText("12.3%/200k")).toBeTruthy();
  });

  it("distinguishes the unknown dash from a zero hit rate", () => {
    const noCacheHit = usage(100, 5, 0, 0);
    render(
      <TopBar
        title="Session"
        model="gpt"
        context={ctx}
        onCompact={() => {}}
        compacting={false}
        pulse={0}
        usage={noCacheHit}
        latest={noCacheHit}
      />,
    );
    expect(screen.getByText("CH0%")).toBeTruthy();
    expect(screen.queryByText("CH—")).toBeNull();
  });

  it("keeps all four badges readable under a long title", () => {
    render(
      <TopBar
        title={"a".repeat(240)}
        model="gpt"
        context={ctx}
        onCompact={() => {}}
        compacting={false}
        pulse={0}
        usage={usage(3396, 101, 3334, 0)}
        latest={usage(3396, 101, 3334, 0)}
      />,
    );
    for (const text of ["↑3.4k", "↓101", "CH98.2%", "12.3%/200k"]) {
      expect(screen.getByText(text)).toBeTruthy();
    }
  });

  it("exposes the cumulative details as a hover tooltip", () => {
    const cumulative = usage(3396, 101, 3334, 0);
    const latest = usage(1000, 101, 0, 0);
    render(
      <TopBar
        title="Session"
        model="gpt"
        context={ctx}
        onCompact={() => {}}
        compacting={false}
        pulse={0}
        usage={cumulative}
        latest={latest}
      />,
    );
    expect(screen.getByTitle(usageTooltip(cumulative, latest))).toBeTruthy();
  });
});
