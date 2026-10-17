# 03: GUI Completion popup

**What to build:** In the GUI input, typing `/` at the start of the buffer opens the Completion popup
above the box. Each keystroke refreshes a fuzzy, best-first candidate list from the host `complete`
IPC; rows show the candidate value and its description; `↑`/`↓` move the selection (auto-tracking the
best match while typing, sticky after a manual move); `Tab` — and `Enter` while the popup is open —
Commit to buffer (write the spelling with a trailing space, do not submit); `Esc` closes the popup
without clearing the input; `Shift+Enter` still inserts a newline; a later `Enter` with the popup
closed submits. The popup sits above the input so the box doesn't jump when it opens/closes.

**Blocked by:** 02 (needs the host `complete` IPC and the filtered GUI pool).

**Status:** ready-for-agent

- [ ] Popup opens only for a leading `/` token (before the first space/newline of the token).
- [ ] Candidates fetched per keystroke from `complete`; rendered as a floating list above the input
      showing value + description.
- [ ] `↑`/`↓` navigation with auto-track-then-sticky selection (ADR-0005 semantics).
- [ ] `Tab` and open-`Enter` Commit to buffer (trailing space, cursor after, no submit); `Esc` closes
      the popup leaving the buffer intact; `Shift+Enter` newline; closed-`Enter` submits.
- [ ] Opening/closing keeps the input box position-stable.
- [ ] Verified by the debug webview screenshot path + manual smoke; `tsc --noEmit` + `vite build`
      clean. (No new frontend test tooling — ranking parity is proven upstream in ticket 02.)
