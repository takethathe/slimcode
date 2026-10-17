# 05: GUI submit executes Commands and injects Skills

**What to build:** Submitting a committed `/` token in the GUI actually does something. The frontend
intercepts the small set of Commands that map onto existing GUI actions and fires them without
starting a model turn: `/new` → new session, `/compact` → compact the current session, `/exit` → close
the window, `/help` → show the command list. Every other submission goes to the host `prompt` path,
which runs the shared Prompt preprocessing seam, so a `/skill:name` trigger sends the expanded `<skill>`
block to the model instead of the literal slash text (the skill's display shown first as an ordinary
user message; a dedicated `[skill]` block is deferred).

**Blocked by:** 01 (the `prepare_user_prompt` seam), 03 (so a token can be committed then submitted to
exercise the path end-to-end).

**Status:** ready-for-agent

- [ ] Frontend intercepts `/new`, `/compact`, `/exit`, `/help` on submit and invokes the existing
      session/compact/close actions and a local command list — no host `prompt` call for these.
- [ ] Host `prompt` runs `prepare_user_prompt`; for a Skill it sends the skill's model text to the
      session; plain text and un-mapped Commands (e.g. `/usage`, `/skills`) fall through unchanged.
- [ ] Controller test: submitting `/skill:name` makes the provider receive the `<skill>` block text,
      not the literal trigger; a plain prompt is unchanged.
- [ ] Manual smoke: commit-then-Enter runs `/new` and injects a skill turn.
- [ ] `cargo test` green; GUI crate gates + `tsc --noEmit` + `vite build` clean.
