import { useEffect, useState } from "react";
import type { GuiSettings } from "../types";

// The settings modal (spec user stories 16-17, ticket 05): edit model /
// base URL / API key / cache. Saving writes `gui.toml` (never `config.toml`);
// the API key renders as a password input.

export function SettingsModal({
  settings,
  onSave,
  onClose,
}: {
  settings: GuiSettings | null;
  onSave: (settings: GuiSettings) => Promise<void>;
  onClose: () => void;
}) {
  const [model, setModel] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [cache, setCache] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (settings) {
      setModel(settings.model ?? "");
      setBaseUrl(settings.base_url ?? "");
      setApiKey(settings.api_key ?? "");
      setCache(settings.cache ?? true);
    }
  }, [settings]);

  if (!settings) return null;

  async function save(): Promise<void> {
    setSaving(true);
    setError(null);
    try {
      await onSave({
        model: model.trim() || null,
        base_url: baseUrl.trim() || null,
        api_key: apiKey.trim() || null,
        cache,
      });
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/30 p-4">
      <div className="w-full max-w-md rounded-xl border border-slate-200 bg-white p-5 shadow-xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-base font-semibold text-slate-800">Settings</h2>
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

        <div className="space-y-3">
          <Field label="Model">
            <input
              value={model}
              onChange={(e) => setModel(e.target.value)}
              placeholder="e.g. qwen-max"
              className="input"
            />
          </Field>
          <Field label="Base URL">
            <input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://…/v1"
              className="input"
            />
          </Field>
          <Field label="API key">
            <input
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-…"
              autoComplete="off"
              className="input"
            />
          </Field>
          <Field label="Explicit caching">
            <label className="flex items-center gap-2 text-sm text-slate-700">
              <input
                type="checkbox"
                checked={cache}
                onChange={(e) => setCache(e.target.checked)}
                className="size-4 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500"
              />
              Enable context caching
            </label>
          </Field>
        </div>

        <p className="mt-3 text-xs text-slate-400">
          These settings are saved to <code>gui.toml</code> and apply to the
          next prompt. They never touch the shared <code>config.toml</code>.
        </p>

        {error ? (
          <p className="mt-2 rounded bg-red-50 px-2 py-1.5 text-xs text-red-700">
            {error}
          </p>
        ) : null}

        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-slate-200 px-3 py-1.5 text-sm font-medium text-slate-600 hover:bg-slate-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={save}
            disabled={saving}
            className="rounded-md bg-indigo-600 px-3 py-1.5 text-sm font-medium text-white shadow-sm hover:bg-indigo-500 disabled:opacity-50"
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs font-medium text-slate-500">
        {label}
      </span>
      {children}
    </label>
  );
}
