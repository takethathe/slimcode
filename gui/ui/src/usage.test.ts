import { describe, expect, it } from "vitest";
import {
  EMPTY_USAGE,
  addUsage,
  cacheHitBadge,
  cacheHitPercent,
  usageSummary,
  usageTooltip,
} from "./usage";
import type { TokenUsage } from "./types";

// A usage with cache details, mirroring the Rust `usage_summary` test vector:
// prompt=10, cached=8, cache_creation=2.
function withCache(
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

function noDetails(prompt: number, completion: number): TokenUsage {
  return {
    prompt_tokens: prompt,
    completion_tokens: completion,
    total_tokens: prompt + completion,
    prompt_tokens_details: null,
  };
}

describe("cacheHitPercent (mirror of Rust usage_summary percent)", () => {
  it("matches the Rust whole-number and one-decimal cases", () => {
    expect(cacheHitPercent(withCache(10, 5, 8, 2))).toBe("80%");
    expect(cacheHitPercent(withCache(3019, 104, 2048, 0))).toBe("67.8%");
    expect(cacheHitPercent(withCache(3019, 0, 3, 0))).toBe("0.1%");
    expect(cacheHitPercent(withCache(4, 0, 3, 0))).toBe("75%");
  });

  it("returns 0% for a zero prompt or absent details", () => {
    expect(cacheHitPercent(EMPTY_USAGE)).toBe("0%");
    expect(cacheHitPercent(noDetails(3, 1))).toBe("0%");
  });
});

describe("usageSummary (verbatim with TUI/CLI usage_summary)", () => {
  it("reproduces the Rust wording for the with-cache vector", () => {
    expect(usageSummary(withCache(10, 5, 8, 2))).toBe(
      "tokens: 10 prompt (8 cached, 80%) + 5 completion = 15 total",
    );
  });

  it("reproduces the Rust wording for the no-details vector", () => {
    expect(usageSummary(noDetails(3, 1))).toBe(
      "tokens: 3 prompt (0 cached, 0%) + 1 completion = 4 total",
    );
  });

  it("reproduces the Rust rounding vector byte-for-byte", () => {
    expect(usageSummary(withCache(3019, 104, 2048, 1605))).toBe(
      "tokens: 3019 prompt (2048 cached, 67.8%) + 104 completion = 3123 total",
    );
  });
});

describe("cacheHitBadge (unknown vs 0%)", () => {
  it("is an em dash when there is no request yet", () => {
    expect(cacheHitBadge(null)).toBe("—");
  });

  it("is an em dash when the endpoint omitted cache details", () => {
    expect(cacheHitBadge(noDetails(10, 5))).toBe("—");
  });

  it("is an em dash for a zero-prompt request", () => {
    expect(cacheHitBadge(withCache(0, 0, 0, 0))).toBe("—");
  });

  it("is 0% when details are present but nothing was cached", () => {
    expect(cacheHitBadge(withCache(10, 5, 0, 2))).toBe("0%");
  });

  it("is the percentage when there is a cache hit", () => {
    expect(cacheHitBadge(withCache(10, 5, 8, 2))).toBe("80%");
    expect(cacheHitBadge(withCache(3019, 104, 2048, 0))).toBe("67.8%");
  });
});

describe("addUsage (cumulative merge)", () => {
  it("sums every field and merges cache details", () => {
    const merged = addUsage(withCache(10, 5, 8, 2), withCache(20, 7, 4, 1));
    expect(merged).toEqual(withCache(30, 12, 12, 3));
  });

  it("keeps details null only when both sides have none", () => {
    expect(addUsage(noDetails(3, 1), noDetails(4, 2)).prompt_tokens_details).toBeNull();
    expect(
      addUsage(noDetails(3, 1), withCache(10, 5, 8, 2)).prompt_tokens_details,
    ).not.toBeNull();
  });

  it("treats an absent side as zero when merging details", () => {
    // A no-details request still contributes its (zero) cache counts to a
    // cumulative that has them.
    const merged = addUsage(withCache(10, 5, 8, 2), noDetails(6, 3));
    expect(merged.prompt_tokens_details).toEqual({
      cached_tokens: 8,
      cache_creation_input_tokens: 2,
    });
    expect(merged.total_tokens).toBe(24);
  });
});

describe("usageTooltip", () => {
  it("labels the badge's latest rate, then R/W counts, then the cumulative summary", () => {
    const cumulative = withCache(18, 9, 12, 2);
    const latest = withCache(10, 5, 8, 2);
    expect(usageTooltip(cumulative, latest)).toBe(
      "latest CH80% · R12 W2 · tokens: 18 prompt (12 cached, 66.7%) + 9 completion = 27 total",
    );
  });

  it("shows the unknown marker for a fresh session", () => {
    expect(usageTooltip(EMPTY_USAGE, null)).toBe(
      "latest CH— · R0 W0 · tokens: 0 prompt (0 cached, 0%) + 0 completion = 0 total",
    );
  });
});
