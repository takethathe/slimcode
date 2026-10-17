# GUI transcript detects `<skill>` blocks from message text; skill activation renders asymmetrically

The GUI transcript used to paint every message as a "You"/"AI" avatar bubble. A
`/skill:name` (or `/{name}`) trigger then showed the backend-expanded `<skill …>` XML
block verbatim — noise only the model needs — and a model that `read` a `SKILL.md` to
activate a skill showed up as an ordinary read card, indistinguishable from reading any
other file. Any leading `/xxx` command looked exactly like a plain prompt. The user asked
for a block-based transcript: no avatars, the user's own input bold on a louder background,
skill activation a clear notice, and command/skill trigger tokens emphasised inline.

Source spec: `.scratch/gui-block-display/spec.md`. Terms: **Skill active block**,
**Transcript chip** (`GLOSSARY.md`). This records *how the GUI reaches those visuals* —
entirely in `gui/ui`, with `gui/src-tauri` (host, `GuiWireEvent`, controller) and the
SDK/app untouched.

## Decisions

### D1 — Detect `<skill>` from message/tool-result text in the frontend, not via a new wire event

A frontend-pure `parseSkillBlock(text)` mirrors `slimcode_app::skills::parse_skill_block`:
it yields `(name, innerContent)` **only when `text` starts with** `<skill name="…"`; a
mid-text mention, an unterminated tag, or plain prose yields `null`. Both activation paths
already carry that prefix — the SDK's `before_tool` hook serves a skill-reading `read`'s
result as the block, and a user trigger reaches the frontend as the block text plus any
trailing task argument after `</skill>` — so one detector covers live streaming *and* a
resumed snapshot, with no activation event added to the wire contract.

### D2 — The two activation paths render asymmetrically

- **User-triggered** (a Prompt): shows as the user's own block with the trigger token
  rebuilt as a `/{name}` **Transcript chip** (teal) plus the trailing task argument; the
  XML is hidden. No separate notice — a Prompt is already the user's message.
- **Model-activated** (a `read`): no user message exists, so its read card is replaced
  **in place** by a dedicated Skill active block — a slate notice reading
  `Skill "<name>" activated`. The same event never shows twice.

A leading `/{token}` on a non-skill user message likewise reads as a **command** chip
(amber), so the two `/` families are colour-distinct. Intercepted local commands
(`/new`, `/usage`, `/compact`, `/help`, `/exit`) never enter the user-message stream, so
they produce no chip. `blocksFromHistory` runs the identical detector over replayed
messages, keeping resume visually equal to live.

### D3 — Lock the detection/reducer logic with a Vitest seam (reverses ADR-0024 D7)

The chip/skill decision lives in the pure transcript reducer, so we introduced **Vitest**
to `gui/ui` (dev dependency + `test` script) to unit-test "given input → which `Block`
list" — the deliberate reversal of ADR-0024 D7's "v1 ships no Vitest", justified because
this parsing/regression logic is worth pinning. Only the reducer is tested; the
`Transcript.tsx` presentation (avatar removal, teal/amber/slate colours, chip DOM) stays
on manual acceptance.

## Considered Options

- **Add a `skill_activated` wire event** — rejected: the backend already puts the `<skill>`
  block in the message/tool-result text, so a new event would widen the wire contract and
  split live-vs-resume handling for zero new information.
- **Emit a standalone notice for the user-trigger path too** — rejected: a trigger *is* a
  user message; double-rendering it would show the same activation twice.
- **Keep avatars and only hide the XML** — rejected: the block model (bold user block,
  borderless assistant flow, in-place skill block) is what makes the three cases legible.

## Consequences

- `gui/ui` only: `transcript.ts` gains `parseSkillBlock`, a `chip` on the `user` block, a
  `skill` block variant, and the reducer/`blocksFromHistory` branches; `Transcript.tsx`
  drops avatars and renders chips + the skill notice.
- `package.json` gains Vitest and a `test` script; the first GUI-side automated test seam.
- The TUI's `[skill] <name>` form, the session log format, and the wire contract are
  unchanged — this is purely a rendering-model decision layered over existing data.
