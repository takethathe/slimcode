// @vitest-environment jsdom
// DOM assertions for the Card panel (ADR-0030 D1/D2/D6/D8): the empty state,
// the collapse rail, per-Card chips, the streaming progress bar, the retry
// affordance, and that one broken element degrades alone.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Spec } from "@json-render/core";

import { CardView } from "./CardView";
import type { CardState } from "../cards";

afterEach(cleanup);

function makeCard(overrides: Partial<CardState> = {}): CardState {
  return {
    id: "result",
    kind: "result",
    spec: {
      root: "root",
      elements: {
        root: {
          type: "Card",
          props: { title: "Result" },
          children: ["body"],
        },
        body: {
          type: "Text",
          props: { content: "The turn is done." },
          children: [],
        },
      },
    },
    applied: 2,
    generating: false,
    failed: false,
    previous: null,
    buffer: "",
    ...overrides,
  };
}

function panel(cards: CardState[], overrides: Partial<Parameters<typeof CardView>[0]> = {}) {
  return render(
    <CardView
      cards={cards}
      generating={false}
      collapsed={false}
      notice={null}
      onToggle={() => {}}
      onGenerate={() => {}}
      {...overrides}
    />,
  );
}

describe("CardView", () => {
  it("says so when no Card exists", () => {
    panel([]);
    expect(screen.getByText("No cards yet")).toBeTruthy();
  });

  it("renders a Card's title and content", () => {
    panel([makeCard()]);
    // The chip and the Card title both read "Result": the heading is the Card.
    expect(screen.getByRole("heading", { name: "Result" })).toBeTruthy();
    expect(screen.getByText("The turn is done.")).toBeTruthy();
  });

  it("shows the kind chip and the streaming progress bar while growing", () => {
    panel([makeCard({ kind: "task-board", id: "task-board", generating: true })]);
    expect(screen.getByText("Task board")).toBeTruthy();
    expect(screen.getByText("streaming")).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: "Card generation progress" })).toBeTruthy();
  });

  it("offers a retry when a generation failed", () => {
    const onGenerate = vi.fn();
    panel([makeCard({ failed: true, spec: { root: "", elements: {} } })], { onGenerate });
    fireEvent.click(screen.getByText("Generation failed — retry"));
    expect(onGenerate).toHaveBeenCalledOnce();
  });

  it("collapses to a rail that hands the width back", () => {
    const onToggle = vi.fn();
    panel([makeCard()], { collapsed: true, onToggle });
    expect(screen.queryByText("The turn is done.")).toBeNull();
    const expand = screen.getByRole("button", { name: "Expand cards" });
    fireEvent.click(expand);
    expect(onToggle).toHaveBeenCalledOnce();
  });

  it("shows a panel-local notice instead of the global banner", () => {
    panel([], { notice: "Card generation is already running." });
    expect(screen.getByText("Card generation is already running.")).toBeTruthy();
  });

  it("degrades one unknown element without taking the Card down", () => {
    const spec: Spec = {
      root: "root",
      elements: {
        root: { type: "Card", props: { title: "Result" }, children: ["body", "odd"] },
        body: { type: "Text", props: { content: "Still here." }, children: [] },
        odd: { type: "Sparkle", props: {}, children: [] },
      },
    };
    panel([makeCard({ spec })]);
    expect(screen.getByText("Still here.")).toBeTruthy();
    expect(screen.getByText("Sparkle unavailable")).toBeTruthy();
  });

  it("asks the host to generate from the header", () => {
    const onGenerate = vi.fn();
    panel([], { onGenerate });
    fireEvent.click(screen.getByRole("button", { name: "Generate cards" }));
    expect(onGenerate).toHaveBeenCalledOnce();
  });
});
