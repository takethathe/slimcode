# GUI shows a resident top-bar Usage HUD; the transcript's usage row is dropped

The GUI had moved the shared `usage_summary` text into the waterfall: the wire `usage`
event is **one per assistant message**, so a turn with two tool calls inserted three
`tokens: … prompt (… cached, …%) + … completion = … total` rows between the prose. Each
row read stale the moment it landed and was overwritten by the next request, so glancing
at "what did this cost, is the cache still working, how much context is left" meant
scrolling back through the transcript — while the top-right corner already held the
context badge's free space. The corner becomes a resident Usage HUD and the transcript
loses the row.

Source spec: `.scratch/gui-usage-hud/spec.md`. Term: **Usage HUD** (`GLOSSARY.md`). This
records *how the fork reaches those visuals* — entirely in `gui/ui`; `gui/src-tauri`
(host, `GuiWireEvent`, controller) and the SDK/app are untouched.

## Decisions

### D1 — Four resident badges; the detail goes to the tooltip

The HUD holds `↑ / ↓ / CH / ctx`. `R` (cache read), `W` (cache write) and the full
`usage_summary` sentence move into the hover tooltip: the corner is a resident HUD, six
inline numbers overload it, and hover is the fallback for exact figures. The tooltip opens
with `latest CH…` because the summary's percentage is the **session cumulative** while the
badge's `CH` is the **latest request** — the same `CH` in two calibres, so the label stops
a reader conflating them.

### D2 — `CH` reads the latest request; the TUI keeps the cumulative — a deliberate fork

`CH` is `cached / prompt` of the most recent request (one decimal, trailing `.0` trimmed),
matching pi's footer. slimcode's TUI footer and `/usage` report the **session-cumulative**
rate. The two answer different questions — the GUI's "is the cache working right now"
versus the TUI's "how much did it save overall" — so both are kept rather than reconciled.
`CH` can only come from the `usage` event: the host snapshot carries a cumulative
`session.usage()` but no per-request figure.

### D3 — The cumulative is not persisted; a resumed session restarts at zero

`Session.usage` is `#[serde(skip)]`, and resume replays only messages and title, so the
HUD honestly reads `↑0 ↓0 CH—` after a resume until a new request arrives. Cross-session
cumulative is a separate concern, tracked in
`.scratch/session-usage-persistence/issues/01-persist-usage-across-resume.md`.

### D4 — Always four, zero included; unknown is not 0%

The empty state is `↑0 ↓0 CH— 0%/200k`. `CH—` means "the endpoint gave no cache detail" or
"a zero-prompt request" (unknown); `CH0%` means "detail present, nothing cached". The
badges never appear or disappear, so their width never jumps.

### D5 — Local accumulate on events, recalibrate on the snapshot

The running total folds each `usage` event in (so `↑/↓` tick up mid-turn) and is
**overwritten** by `session.usage()` on every host-snapshot refresh. The refresh triggers
are turn end, message, `compacted`, and new / resume / delete — `compacted` matters because
it is the *only* event a manual compaction emits, while the summary request is already
folded into the host total; without it `↑/↓` and the context badge keep the pre-compaction
values until the next turn ends.

The overwrite needs a fence, because the two channels are not ordered against each other:
a snapshot requested before a `usage` event lands carries the smaller total and would roll
the HUD backwards. Every local delta bumps a monotonic epoch; a refresh records the epoch
when it is issued and applies `session.usage()` only if the epoch is unchanged when the
answer arrives. A session swap resets the total and the epoch locally rather than relying
on the next snapshot to zero it.

### D6 — Delete the render, keep the event

The transcript block model drops the `usage` member and its append entry, and
`UsageLine.tsx` is deleted. `GuiWireEvent::Usage` and the Rust `DisplayItem::Usage` stay —
their job changes from "paint a row" to "feed the HUD". The reducer keeps an empty `usage`
branch so the `switch` stays exhaustive over every wire kind. Zero Rust change.

### D7 — `/usage` degrades to calibrate + pulse

`/usage` stays in the frontend-intercepted set (never sent to the model). Its behaviour is
now: pull one snapshot to recalibrate the total, and fire a one-shot ~600ms ring on the HUD
as feedback; under `prefers-reduced-motion: reduce` the ring degrades to a static outline.
It appends no transcript block.

### D8 — The tooltip text is mirrored in the frontend, not carried on the wire

TypeScript mirrors `usage_summary` / `cache_hit_percent`, guarded by unit tests that copy
the Rust cases so the wording cannot drift between frontends. The `latest CH{pct}` prefix
is the GUI's own annotation and stays out of the mirror assertions. The sentence is not put
on the wire, because that would repeat one fixed template on every assistant message for a
display-only string.

## Considered Options

- **Keep the usage row in the transcript** — rejected: a turn with N tool calls inserts
  N+1 stale measurement rows between the prose.
- **Six inline badges (add `R` and `W`)** — rejected: the corner is a resident HUD; hover
  already carries the detail.
- **Report the cumulative rate in `CH` to match the TUI** — rejected: the badge answers "is
  the cache working now", which a cumulative average masks.
- **Send the summary sentence over the wire** — rejected: it repeats a fixed template on
  every assistant message for a string the GUI can derive itself.

## Consequences

- `gui/ui` only: a new pure `usage.ts` (cumulative merge, latest rate, formatting, summary
  and tooltip text); `TopBar.tsx` renders the four badges, the tooltip and the `/usage`
  ring; `App.tsx` holds the cumulative / latest / pulse state and updates it at the event
  and snapshot seams; `transcript.ts` drops the `usage` block; `UsageLine.tsx` is deleted;
  `index.css` gains the pulse keyframes and the reduced-motion fallback.
- The wire contract (`GuiWireEvent::Usage`, `SessionSnapshot.usage`) and the whole
  TUI / CLI / SDK / FFI stack are unchanged — this is purely a GUI display decision.
- A resumed session's HUD reads zero until the next request (D3), the known boundary
  deferred to session-usage-persistence.
