# GUI assistant prose renders markdown, and keeps the heading markers

The GUI's assistant block used to be a `<pre>`: the model's `**bold**` showed as literal
asterisks, a markdown table as rows of pipes, a fence as three backticks, and `###` looked
exactly like a paragraph. Model answers are structured documents — sections, tables, code,
lists — and the GUI could only be read as plain text.

The TUI already has real markdown (`crates/tui/src/markdown.rs`, `pulldown-cmark`), but it
**drops** the heading `#` and carries level only through colour and one heading style. For a
reader that is not enough to tell "level 2" from "level 3". Copying the TUI would inherit the
exact defect the user is complaining about, so the GUI deliberately diverges.

Source spec: `.scratch/gui-markdown/spec.md`. Term: **Heading marker** (`GLOSSARY.md`).

## Decisions

### D1 — Parse with `react-markdown` + `remark-gfm`, at render time, over the existing block model

The transcript's block model keeps holding **raw text**; markdown is parsed in the render layer
by a single `Markdown` component. Live streaming appends chunks to the same block text and the
component re-parses the whole block per chunk (the same per-frame re-parse the TUI does), so a
resumed session renders through exactly the same function as a live one — no new wire field, no
new `Block` variant, and `transcript.ts` untouched.

A real parser is used because the value being bought is CommonMark/GFM correctness (adjacent
inline emphasis, nested lists, table alignment) that a hand-rolled parser gets wrong for years.
`react-markdown` maps nodes to React elements instead of injecting HTML, and `skipHtml` drops
bare HTML, so model output is never trusted as markup and no sanitizer dependency is needed.

### D2 — Headings keep `#`×N as a literal prefix; font size is decoration in two tiers only

Each heading renders as a real `h1`–`h6` whose content is preceded by its `#`×N marker in dim
monospace. The marker count is the hierarchy's **primary** signal. Size distinguishes only two
tiers — h1/h2 a step above body text, h3 and deeper at body size — so a size difference can never
imply a level the `#` count does not.

This knowingly diverges from the TUI/pi behaviour (which drops the marker). The TUI is left
alone; the inconsistency is intentional and recorded here so a future reader does not "fix" the
GUI back into the same defect.

### D3 — Prose in the UI font, code in the monospace font; code blocks are structured, not highlighted

Assistant prose moves to the GUI's sans stack; inline code and fenced blocks stay monospace. That
contrast is most of markdown's readability gain. A fenced block renders as a bordered grey panel
whose one-line scroll container keeps long lines from wrapping (and from widening the transcript);
the fence's info string appears as a language label in the corner, and a fence without one shows
no label. Syntax highlighting and a copy button are deliberately out: the first is a dependency
decision of its own, the second an interaction decision.

### D4 — `http(s)` links open in the system browser through `tauri-plugin-opener`; every other target is inert

The webview must never navigate away from the conversation, so the link click always calls
`preventDefault`. Only an `http(s)` target is handed to the host's opener plugin and thus to the
system browser; a relative path or an anchor keeps the link's styling but does nothing. The
open-external call is isolated in one small frontend module so the rendering tests can stub it
instead of faking a Tauri runtime. A middle-click or "open in new tab" fires `auxclick` rather
than `click`, so the link cancels that too — see D7 for the host-side backstop that catches any
pointer path the DOM handler misses.

### D5 — GFM's task-list boxes and footnotes degrade to text

`remark-gfm` parses two constructs this seam will not render. A task list would otherwise put a
disabled `<input>` — a form control — in the middle of prose, and a footnote emits an
auto-generated "Footnotes" heading that the model never wrote, which breaks the marker count as
the hierarchy's signal, plus an `#`-anchor the inert link handler can never open. Both are
rewritten at the mdast level before rendering: a task item loses its `checked` flag and gains a
`☐`/`☑` text prefix, and a footnote reference becomes the literal `[^id]` text while its
definition is dropped.

### D6 — The rendering layer gets DOM tests (jsdom + Testing Library)

ADR-0024 D7's "no frontend tests" was already reversed once, for the pure reducer (ADR-0027 D3);
this slice reverses its other half — "rendering stays on manual acceptance". The facts this
change is judged on are DOM facts (`##` still visible, table cells present, label present or
absent, click opens once), and the most likely regression (a component mapping silently dropping
the marker prefix) is invisible to a reducer test. Rendering tests use a per-file jsdom
environment so the pure reducer tests keep running under the node environment.

### D7 — The host enforces "never navigate" as a runtime backstop, not just a frontend convention

The frontend cancels link activation on every pointer path, but that is only a convention: the
webview's CSP is `null`, so nothing else stops a navigation the DOM handler failed to prevent (a
middle-click, a stray script-driven redirect). The trusted bound is therefore moved host-side. A
`nav-guard` plugin in `gui/src-tauri/src/lib.rs` registers an `on_navigation` allowlist that
returns true only when the target URL shares the webview's own origin, and cancels everything
else — an `http(s)` page, `file://`, any cross-origin target. External links are unaffected
because they leave through the opener plugin (D4), never as webview navigations. It is registered
as a plugin because Tauri v2 exposes `on_navigation` only on the webview/plugin builders, while
the window itself is declared in `tauri.conf.json`.

"Shares the webview's own origin" is decided by comparing scheme, host and port — **not**
`Url::origin()`. The bundled app is served from a custom protocol (`tauri://localhost` on
macOS/Linux), a non-special scheme for which the `url` crate returns a freshly-generated *opaque*
origin that is only equal to itself; two parses of the same `tauri://localhost` URL therefore never
compare equal, so an origin-equality check cancels even the initial page load and ships a blank
release window (dev hides it because `http://localhost` has a comparable tuple origin). The check
lives in the pure `navigation_allowed` function, covered by `gui/src-tauri/tests/nav_guard.rs`.

## Considered Options

- **Hand-rolled zero-dependency parser** — rejected: GFM tables, inline emphasis pairing and
  nested lists are where naive parsers fail, and this is exactly the content models emit.
- **`marked` + `DOMPurify` + `innerHTML`** — rejected: fewer direct dependencies but the HTML
  string path needs its own sanitizer and loses the node-level control the heading marker needs.
- **Drop the `#` like the TUI** — rejected: that is the reported defect; size alone cannot carry
  heading level.
- **Keep the trailing `▌` streaming caret inline** — rejected: appended to raw markdown it would
  corrupt the last line (`**bold▌`); the transcript already shows a `Working…` row, so the caret
  was dropped rather than repositioned.
- **Render plain text while streaming, switch to markdown on completion** — rejected: the whole
  block would jump under the reader's eyes at the end of every turn.
- **Styled but unclickable links** — rejected: a link that looks live and does nothing reads as a
  bug, and the opener plugin is a one-time host-side cost.

## Consequences

- `gui/ui` runtime dependencies gain `react-markdown`, `remark-gfm` and
  `@tauri-apps/plugin-opener`; dev dependencies gain `jsdom` and Testing Library. This is the
  largest frontend dependency change in the repo so far, and it is bundle-size cost paid for
  CommonMark/GFM correctness.
- The Tauri host registers the opener plugin and grants `opener:allow-open-url` scoped to
  `https://*` / `http://*` in its capability — not plugin default, which also carries the
  file-manager reveal permission.
- The Tauri host also registers a `nav-guard` plugin whose `on_navigation` allowlist lets the
  webview navigate only to its own origin, enforcing the never-navigate invariant at runtime
  under `csp: null` (D7).
- `Transcript.tsx`'s assistant branch renders through the markdown component and no longer paints
  a streaming caret; user blocks, reasoning, tool cards, usage lines, compaction notices and error
  blocks are unchanged.
- The TUI's heading rendering is unchanged, so the two frontends now differ on purpose.
- Every markdown display rule lives in one component, which is also the test seam: new constructs
  are added by extending its node mapping plus one DOM assertion.
