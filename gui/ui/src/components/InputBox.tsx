import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { INTERCEPTED_COMMANDS } from "../commands";
import type { CompletionItem } from "../types";

// The bottom input bar: a multi-line textarea with Enter-to-send and
// Shift+Enter for a newline (spec user story 21), plus the send / stop buttons
// (running disables send and enables stop).
//
// Typing a leading `/` token opens the Completion popup above the box — the
// same fuzzy, best-first Commands + Skills list the TUI shows, fetched per
// keystroke from the host `complete` IPC (ADR-0026). `↑`/`↓` move the selection
// (auto-tracking the top match while typing, sticky by value after a manual
// move — ADR-0005); `Tab` — and `Enter` while the popup is open — Commit to
// buffer (write the spelling with a trailing space, no submit); `Esc` closes the
// popup leaving the buffer intact; a committed, recognised token shows as a
// Command chip; `Shift+Enter` still inserts a newline; `Enter` with the popup
// closed submits. On submit the effect-mapped Commands (the shared
// INTERCEPTED_COMMANDS set) fire their GUI actions instead of starting a
// model turn.

export function InputBox({
  running,
  onSend,
  onStop,
  disabled,
  onNewSession,
  onCompact,
  onUsage,
  onExit,
  onHelp,
}: {
  running: boolean;
  onSend: (text: string) => void;
  onStop: () => void;
  disabled: boolean;
  onNewSession: () => void;
  onCompact: () => void;
  onUsage: () => void;
  onExit: () => void;
  onHelp: () => void;
}) {
  const [text, setText] = useState("");
  const [candidates, setCandidates] = useState<CompletionItem[]>([]);
  const [dismissedToken, setDismissedToken] = useState<string | null>(null);
  const [selectedValue, setSelectedValue] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);

  // The leading `/` token: the first run of non-whitespace, but only when the
  // buffer starts with `/`. An empty string means no completion context.
  const token = useMemo(() => {
    const ws = text.search(/\s/);
    const first = ws === -1 ? text : text.slice(0, ws);
    return first.startsWith("/") ? first : "";
  }, [text]);

  // Fetch candidates per keystroke while a `/` token is being typed.
  useEffect(() => {
    if (!token) {
      setCandidates([]);
      return;
    }
    let active = true;
    api
      .complete(token)
      .then((items) => {
        if (active) setCandidates(items);
      })
      .catch(() => {
        if (active) setCandidates([]);
      });
    return () => {
      active = false;
    };
  }, [token]);

  // The popup is open only while actively typing the token (no trailing
  // whitespace yet) and it has not been dismissed for this exact token.
  const typingToken = text === token;
  const dismissed = dismissedToken !== null && dismissedToken === token;
  const popupOpen =
    !!token && typingToken && !dismissed && candidates.length > 0 && !running && !disabled;

  // Auto-track the top match, or keep the manual pick sticky by value until it
  // is filtered out (ADR-0005).
  const trackedIndex = useMemo(() => {
    if (selectedValue) {
      const idx = candidates.findIndex((c) => c.value === selectedValue);
      if (idx >= 0) return idx;
    }
    return 0;
  }, [candidates, selectedValue]);

  // The Command chip: a recognised, committed token shown while the popup is
  // closed (display-only; the buffer stays plain text).
  const chip = useMemo(() => {
    if (!token || popupOpen) return null;
    const match = candidates.find((c) => c.value === token);
    return match ? { value: match.value, description: match.description } : null;
  }, [token, popupOpen, candidates]);

  // Focus the input on mount and whenever the send path re-enables after a
  // turn, so the user can keep typing without clicking (ticket 07).
  useEffect(() => {
    ref.current?.focus();
  }, [running, disabled]);

  const reset = useCallback(() => {
    setText("");
    setCandidates([]);
    setSelectedValue(null);
    setDismissedToken(null);
  }, []);

  // Commit the selected candidate into the buffer with a trailing space,
  // without submitting, so an argument can follow. Only fires while the popup
  // is open (the buffer is exactly the token), so any trailing text is empty.
  const commit = useCallback((item: CompletionItem) => {
    setText(`${item.value} `);
    setSelectedValue(null);
    setDismissedToken(null);
    requestAnimationFrame(() => ref.current?.focus());
  }, []);

  function move(delta: number): void {
    if (candidates.length === 0) return;
    const next = (trackedIndex + delta + candidates.length) % candidates.length;
    setSelectedValue(candidates[next].value);
  }

  function submit(): void {
    const value = text.trim();
    if (!value || running || disabled) return;
    const command = value.split(/\s+/)[0].toLowerCase();
    reset();
    if (INTERCEPTED_COMMANDS.has(command)) {
      if (command === "/new") onNewSession();
      else if (command === "/compact") onCompact();
      else if (command === "/usage") onUsage();
      else if (command === "/exit") onExit();
      else onHelp();
      return;
    }
    onSend(value);
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>): void {
    const composing = e.nativeEvent.isComposing;
    if (e.key === "Enter" && e.shiftKey) return; // newline, always
    if (popupOpen && !composing) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        move(1);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        move(-1);
        return;
      }
      if (e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        const item = candidates[trackedIndex];
        if (item) commit(item);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissedToken(token);
        return;
      }
    }
    if (e.key === "Enter" && !composing) {
      e.preventDefault();
      submit();
    }
  }

  return (
    <div className="relative shrink-0 border-t border-slate-200 bg-white px-4 py-3">
      {popupOpen ? (
        <div className="absolute bottom-full left-4 right-4 z-10 mb-1 overflow-hidden rounded-lg border border-slate-200 bg-white shadow-lg">
          <ul className="max-h-60 overflow-y-auto py-1" role="listbox" aria-label="Commands">
            {candidates.map((item, i) => (
              <li key={item.value}>
                <button
                  type="button"
                  role="option"
                  aria-selected={i === trackedIndex}
                  onClick={() => commit(item)}
                  className={`flex w-full items-baseline gap-3 px-3 py-1.5 text-left text-sm ${
                    i === trackedIndex ? "bg-indigo-50" : "hover:bg-slate-50"
                  }`}
                >
                  <span className="shrink-0 font-mono text-indigo-700">{item.value}</span>
                  <span className="min-w-0 flex-1 truncate text-xs text-slate-500">
                    {item.description}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {chip ? (
        <div className="pointer-events-none absolute bottom-full left-4 z-10 mb-1 flex items-center gap-2">
          <span className="rounded-md bg-indigo-100 px-2 py-0.5 font-mono text-xs font-medium text-indigo-700 ring-1 ring-inset ring-indigo-200">
            {chip.value}
          </span>
          {chip.description ? (
            <span className="text-xs text-slate-400">{chip.description}</span>
          ) : null}
        </div>
      ) : null}
      <div className="mx-auto flex max-w-3xl items-end gap-2">
        <textarea
          ref={ref}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          rows={1}
          placeholder="Type a prompt… (/ for commands, Enter to send, Shift+Enter for a newline)"
          disabled={disabled}
          className="max-h-48 min-h-10 flex-1 resize-none rounded-lg border border-slate-200 bg-white px-3 py-2 font-mono text-sm text-slate-800 shadow-sm outline-none placeholder:font-sans placeholder:text-slate-400 focus:border-indigo-300 focus:ring-2 focus:ring-indigo-100 disabled:opacity-50"
        />
        {running ? (
          <button
            type="button"
            onClick={onStop}
            className="rounded-lg border border-red-200 bg-red-50 px-4 py-2 text-sm font-medium text-red-700 hover:bg-red-100"
          >
            Stop
          </button>
        ) : (
          <button
            type="button"
            onClick={submit}
            disabled={disabled || !text.trim()}
            className="rounded-lg bg-indigo-600 px-4 py-2 text-sm font-medium text-white shadow-sm hover:bg-indigo-500 disabled:opacity-40"
          >
            Send
          </button>
        )}
      </div>
    </div>
  );
}
