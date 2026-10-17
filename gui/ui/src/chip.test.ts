import { describe, expect, it } from "vitest";
import { chipCategoryForCommitted } from "./chip";

// The input-side Command chip shares the Transcript chip's colour categories,
// derived from the committed candidate spelling.

describe("chipCategoryForCommitted", () => {
  it("reads a `/skill:` spelling as a skill", () => {
    expect(chipCategoryForCommitted("/skill:grill")).toBe("skill");
  });

  it("reads any other recognised token as a command", () => {
    expect(chipCategoryForCommitted("/help")).toBe("command");
    expect(chipCategoryForCommitted("/compact")).toBe("command");
  });

  it("does not misclassify a bare name that merely contains `skill`", () => {
    expect(chipCategoryForCommitted("/skillify")).toBe("command");
  });
});
