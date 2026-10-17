# 01: Lift skill injection into a shared `prepare_user_prompt` seam, migrate the CLI

**What to build:** A single app-layer Prompt preprocessing seam decides what a turn actually sends: a
plain prompt, or a Skill trigger expanded into its `<skill>` block (deduped against the message
history). The CLI one-shot and the interactive TUI both consume it, so `/skill:name` behaves
identically everywhere and the two private copies of the logic collapse into one. Behaviour is
preserved: a submitted skill trigger still reaches the model as the skill block, a skill already
loaded earlier in the conversation still degrades to the already-loaded notice, and non-skill input
passes through. This is the prefactor that later lets the GUI inject skills at all (ADR-0026).

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] A pure `prepare_user_prompt(input, skills, history)` in `slimcode-app` returns the small enum
      (from the spec/ADR-0026 decision):
      ```rust
      pub enum PreparedPrompt {
          Prompt(String),                       // plain text; unrecognised leading /xxx is normalized
          Skill { name, model_text, display },  // model_text → SDK prompt; display → frontend block
      }
      ```
- [ ] It composes the existing primitives: parse a leading skill trigger, resolve it, render the skill
      block with `already_loaded` computed from `history`; a miss returns the normalized plain prompt.
- [ ] Command recognition is NOT part of this seam — Commands stay frontend effects.
- [ ] The CLI one-shot entry and the TUI skill path both call the seam (one-shot passes an empty
      history, reproducing its prior `already_loaded: false`); the two local copies are removed.
- [ ] Seam unit tests cover: skill hit with/without argument, already-loaded vs fresh, plain prompt,
      unrecognised leading `/xxx`, empty vs populated history.
- [ ] Existing CLI skill / one-shot tests still pass; `cargo test` green; `architecture.rs`
      dependency matrix still passes; fmt + clippy `-D warnings` clean.
