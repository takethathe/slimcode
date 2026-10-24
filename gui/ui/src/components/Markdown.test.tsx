// @vitest-environment jsdom
// DOM assertions for the markdown rendering seam: the `<Markdown>` component is
// the only place assistant prose becomes visible markup (spec
// .scratch/gui-markdown/spec.md). Tests assert what a reader can see — text and
// structure — never Tailwind classes or the component tree.
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { openExternal } from "../external";
import { Markdown } from "./Markdown";

// The only module that touches the Tauri runtime is stubbed here, so the link
// tests never need a webview while still asserting the real click handling.
vi.mock("../external", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../external")>();
  return { ...actual, openExternal: vi.fn() };
});

// Vitest runs without globals, so Testing Library's auto-cleanup does not kick
// in: unmount between tests or the queries see the previous render too.
afterEach(cleanup);

beforeEach(() => {
  vi.mocked(openExternal).mockClear();
});

describe("Markdown", () => {
  it("renders bold emphasis without leaking the source markers", () => {
    const { container } = render(<Markdown text="a **bold** word" />);
    expect(screen.getByText("bold").tagName).toBe("STRONG");
    expect(container.querySelector("p")?.textContent).toBe("a bold word");
    expect(container.textContent).not.toContain("**");
  });

  it("renders italic emphasis", () => {
    render(<Markdown text="a *slanted* word" />);
    expect(screen.getByText("slanted").tagName).toBe("EM");
  });

  it("renders inline code without the backticks", () => {
    const { container } = render(<Markdown text="use `cargo test` here" />);
    expect(screen.getByText("cargo test").tagName).toBe("CODE");
    expect(container.textContent).toBe("use cargo test here");
  });

  it("renders unordered and ordered lists as list items", () => {
    const { container } = render(
      <Markdown text={"- one\n- two\n\n1. first\n2. second"} />,
    );
    expect(container.querySelectorAll("ul > li")).toHaveLength(2);
    expect(container.querySelectorAll("ol > li")).toHaveLength(2);
    expect(screen.getByText("one").tagName).toBe("LI");
    expect(screen.getByText("second").tagName).toBe("LI");
  });

  it("renders a blockquote", () => {
    const { container } = render(<Markdown text={"> quoted line"} />);
    expect(container.querySelector("blockquote")?.textContent).toContain(
      "quoted line",
    );
  });

  it("drops bare HTML instead of rendering it as markup", () => {
    const { container } = render(<Markdown text={"<span>raw</span> ok"} />);
    expect(container.querySelector("span")).toBeNull();
    expect(container.textContent).toContain("raw");
    expect(container.textContent).not.toContain("<span>");
  });

  it("keeps the heading markers so the # count carries the hierarchy", () => {
    const { container } = render(
      <Markdown text={"# One\n\n## Two\n\n### Three\n\n#### Four"} />,
    );
    expect(container.querySelector("h1")?.textContent).toBe("# One");
    expect(container.querySelector("h2")?.textContent).toBe("## Two");
    expect(container.querySelector("h3")?.textContent).toBe("### Three");
    expect(container.querySelector("h4")?.textContent).toBe("#### Four");
  });

  it("renders a fenced code block with its language label", () => {
    const { container } = render(
      <Markdown text={"```rust\nfn main() {}\n```"} />,
    );
    const pre = container.querySelector("pre");
    expect(pre?.querySelector("code")?.textContent?.trim()).toBe("fn main() {}");
    expect(pre?.getAttribute("data-language")).toBe("rust");
    expect(screen.getByText("rust")).toBeTruthy();
  });

  it("omits the language label for a fence without an info string", () => {
    const { container } = render(<Markdown text={"```\nalpha\n```"} />);
    const pre = container.querySelector("pre");
    expect(pre?.querySelector("code")?.textContent?.trim()).toBe("alpha");
    expect(pre?.hasAttribute("data-language")).toBe(false);
  });

  it("renders a GFM table instead of the pipe syntax", () => {
    const { container } = render(
      <Markdown text={"| name | value |\n| --- | --- |\n| alpha | 1 |"} />,
    );
    expect(container.querySelector("table")).not.toBeNull();
    expect(container.querySelectorAll("thead th")).toHaveLength(2);
    expect(container.querySelectorAll("tbody td")).toHaveLength(2);
    expect(screen.getByText("alpha").tagName).toBe("TD");
    expect(container.textContent).not.toContain("|");
  });

  it("opens http(s) links in the system browser without navigating", () => {
    const { container } = render(
      <Markdown text="see [docs](https://example.com/guide)" />,
    );
    const link = screen.getByText("docs");
    const navigated = fireEvent.click(link);
    // A middle-click / "open in new tab" fires `auxclick`, not `click`; it must
    // be cancelled too, or the webview navigates away from the conversation.
    const auxNavigated = fireEvent(
      link,
      new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }),
    );

    expect(openExternal).toHaveBeenCalledTimes(1);
    expect(openExternal).toHaveBeenCalledWith("https://example.com/guide");
    expect(container.querySelector("a")?.getAttribute("href")).toBe(
      "https://example.com/guide",
    );
    expect(navigated).toBe(false);
    expect(auxNavigated).toBe(false);
  });

  it("leaves non-http(s) link targets inert", () => {
    render(<Markdown text="see [guide](./guide.md)" />);
    fireEvent.click(screen.getByText("guide"));
    expect(openExternal).not.toHaveBeenCalled();
  });

  it("never loads a remote image, showing its alt text instead", () => {
    const { container } = render(
      <Markdown text="before ![diagram](https://example.com/tracker.png)" />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("diagram");
  });

  it("renders task list boxes as text, not form controls", () => {
    const { container } = render(<Markdown text={"- [ ] todo\n- [x] done"} />);
    expect(container.querySelector("input")).toBeNull();
    expect(container.textContent).toContain("☐ todo");
    expect(container.textContent).toContain("☑ done");
  });

  it("leaves footnotes out of the rendered prose", () => {
    const { container } = render(
      <Markdown text={"see the note[^1]\n\n[^1]: the detail"} />,
    );
    expect(container.querySelector("section")).toBeNull();
    expect(container.textContent).not.toContain("Footnotes");
    expect(container.textContent).toContain("see the note[^1]");
  });
});
