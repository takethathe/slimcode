import { useEffect, useRef, useState } from "react";

// The bottom input bar: a multi-line textarea with Enter-to-send and
// Shift+Enter for a newline (spec user story 21), plus the send / stop
// buttons (running disables send and enables stop).

export function InputBox({
  running,
  onSend,
  onStop,
  disabled,
}: {
  running: boolean;
  onSend: (text: string) => void;
  onStop: () => void;
  disabled: boolean;
}) {
  const [text, setText] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);

  // Focus the input on mount and whenever the send path re-enables after a
  // turn, so the user can keep typing without clicking (ticket 07).
  useEffect(() => {
    ref.current?.focus();
  }, [running, disabled]);

  function submit(): void {
    const value = text.trim();
    if (!value || running || disabled) return;
    setText("");
    onSend(value);
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>): void {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      submit();
    }
  }

  return (
    <div className="shrink-0 border-t border-slate-200 bg-white px-4 py-3">
      <div className="mx-auto flex max-w-3xl items-end gap-2">
        <textarea
          ref={ref}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          rows={1}
          placeholder="Type a prompt… (Enter to send, Shift+Enter for a newline)"
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
