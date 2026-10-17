# 04: GUI Command chip

**What to build:** Once a `/` token is committed and the popup is closed, the GUI draws a Command chip
over the leading token — a highlighted, display-only badge that reads as "this is a Command, not my
prose" — for both built-in Commands and `/skill:name` triggers. The underlying buffer stays editable
plain text; the chip is purely visual. Recognition reuses the host `complete` IPC (exact-`value` match
for the leading token), so no new endpoint is added.

**Blocked by:** 03 (popup/commit context established; recognition reuses `complete`).

**Status:** ready-for-agent

- [ ] When the popup is closed and the leading token equals an exact candidate value, the token is
      rendered as a Command chip (Tailwind-styled highlight above the GUI's palette).
- [ ] Chip applies to Commands and `/skill:name` alike.
- [ ] Chip is display-only: editing, deleting, and re-opening the popup on further typing all still
      work because the buffer remains plain text.
- [ ] Chip disappears as soon as the leading token is no longer an exact recognised value.
- [ ] Verified by manual smoke + debug webview screenshot; `tsc --noEmit` + `vite build` clean.
