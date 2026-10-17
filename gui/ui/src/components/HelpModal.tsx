import { INTERCEPTED_COMMANDS } from "../commands";
import type { CompletionItem } from "../types";

// The `/help` command list (ADR-0026 D3): the Commands and Skills the GUI offers.
// The rows come straight from the host `complete("/")` IPC — the same single
// source that feeds the Completion popup (user story 27: the registry stays the
// one source of truth, so this list never drifts from what is actually
// applicable). The Commands the frontend intercepts (the shared
// INTERCEPTED_COMMANDS set) fire a GUI action instead of starting a model turn;
// `/skills` still passes through to the model as plain prompt text (accepted
// scope for this slice).

export function HelpModal({
  items,
  onClose,
}: {
  items: CompletionItem[];
  onClose: () => void;
}) {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/30 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md rounded-xl border border-slate-200 bg-white p-5 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-base font-semibold text-slate-800">Commands</h2>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
            aria-label="Close"
          >
            <svg className="size-4" viewBox="0 0 20 20" fill="currentColor">
              <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
            </svg>
          </button>
        </div>
        <dl className="max-h-80 space-y-2 overflow-y-auto">
          {items.map(({ value, description }) => (
            <div key={value} className="flex items-baseline gap-3">
              <dt className="shrink-0 font-mono text-sm text-indigo-700">{value}</dt>
              <dd className="min-w-0 flex-1 text-sm text-slate-500">
                {description}
                {INTERCEPTED_COMMANDS.has(value) ? (
                  <span className="ml-2 text-xs text-slate-400">· runs locally</span>
                ) : null}
              </dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}
