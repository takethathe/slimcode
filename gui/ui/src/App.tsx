import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, errorMessage } from "./api";
import {
  blocksFromHistory,
  newBlockSeq,
  reduceTranscript,
  type Block,
} from "./transcript";
import { addUsage, EMPTY_USAGE } from "./usage";
import type {
  CompletionItem,
  ContextInfo,
  GuiSettings,
  GuiWireEvent,
  SessionInfo,
  TokenUsage,
} from "./types";
import { Sidebar } from "./components/Sidebar";
import { TopBar } from "./components/TopBar";
import { Transcript } from "./components/Transcript";
import { InputBox } from "./components/InputBox";
import { SettingsModal } from "./components/SettingsModal";
import { HelpModal } from "./components/HelpModal";

export default function App() {
  const [blocks, setBlocks] = useState<Block[]>([]);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [title, setTitle] = useState<string | null>(null);
  const [model, setModel] = useState("");
  const [context, setContext] = useState<ContextInfo | null>(null);
  // The Usage HUD state (spec .scratch/gui-usage-hud): the session cumulative
  // and the most recent request's usage. `usage` is fed by both live `usage`
  // events and the host snapshot (which overwrites it to recalibrate); `CH`
  // comes only from `latestUsage`, the last request seen, since the snapshot
  // carries no per-request figure (spec D2/D5).
  const [usage, setUsage] = useState<TokenUsage>(EMPTY_USAGE);
  const [latestUsage, setLatestUsage] = useState<TokenUsage | null>(null);
  // Bumping this counter asks the HUD to ring once (the `/usage` feedback, spec
  // D7); the value itself is meaningless, only its changes drive the pulse.
  const [usagePulse, setUsagePulse] = useState(0);
  const [warning, setWarning] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [busy, setBusy] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [helpItems, setHelpItems] = useState<CompletionItem[]>([]);
  const [settings, setSettings] = useState<GuiSettings | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The pending-turn event queue: events stream in via the Channel callback
  // (outside React), are reduced through a ref, and flushed into state on a
  // rAF so streaming stays smooth without re-rendering per fragment.
  const eventsRef = useRef<GuiWireEvent[]>([]);
  const flushScheduled = useRef(false);
  // Monotonic fence for the cumulative HUD total: every local `usage` delta
  // bumps it, and a snapshot requested before that bump is discarded when it
  // lands — otherwise an older total would overwrite a newer one (spec D5).
  const usageEpoch = useRef(0);

  const refreshSessions = useCallback(async () => {
    try {
      setSessions(await api.listSessions());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const refreshSnapshot = useCallback(async () => {
    const at = usageEpoch.current;
    try {
      const snapshot = await api.getSession();
      setCurrentId(snapshot.id);
      setTitle(snapshot.title);
      setModel(snapshot.model);
      setContext(snapshot.context);
      // Recalibrate only when no `usage` event arrived while the request was
      // in flight; the host already folded that request into its total, and a
      // local delta may have landed on top of it meanwhile.
      if (at === usageEpoch.current) setUsage(snapshot.usage);
      setWarning(snapshot.warning);
      return snapshot;
    } catch (e) {
      setError(errorMessage(e));
      return null;
    }
  }, []);

  const flushEvents = useCallback(() => {
    flushScheduled.current = false;
    const pending = eventsRef.current;
    if (pending.length === 0) return;
    eventsRef.current = [];
    setBlocks((prev) => reduceTranscript(prev, pending));
    // Feed the HUD from `usage` events: fold each request into the running
    // total so ↑/↓ tick up mid-turn, and remember the latest for `CH`. The
    // snapshot refresh below overwrites the total to recalibrate (spec D5).
    let hudDelta: TokenUsage | null = null;
    let hudLatest: TokenUsage | null = null;
    for (const e of pending) {
      if (e.kind === "usage") {
        hudDelta = hudDelta ? addUsage(hudDelta, e.usage) : e.usage;
        hudLatest = e.usage;
      }
    }
    if (hudDelta) {
      // Bump the fence before the update: any snapshot already in flight was
      // taken against the previous total and is now stale.
      usageEpoch.current += 1;
      setUsage((prev) => addUsage(prev, hudDelta));
    }
    if (hudLatest) setLatestUsage(hudLatest);
    // The authoritative end of a turn: `turn_finished` (completed/cancelled)
    // or `error` (failed). Either one releases the running state.
    if (
      pending.some(
        (e) => e.kind === "turn_finished" || e.kind === "error",
      )
    ) {
      setRunning(false);
    }
    // `message` events drive the title/model/context refresh (ticket 06)
    // and the sidebar row (title / time / message count) refresh (spec:
    // message events drive both the sidebar and the top bar).
    // `compacted` is the only event a manual compaction emits, and the host has
    // already folded the summary request into the session usage by then, so it
    // must recalibrate too — otherwise ↑/↓ and the context badge keep the
    // pre-compaction values until the next turn ends.
    const last = pending[pending.length - 1];
    if (
      pending.some((e) => e.kind === "compacted") ||
      (last && (last.kind === "message" || last.kind === "turn_finished"))
    ) {
      void refreshSnapshot();
      void refreshSessions();
    }
  }, [refreshSnapshot, refreshSessions]);

  // A session swap replaces the whole counter: reset locally and fence off any
  // snapshot taken against the previous session instead of relying on the
  // next snapshot to zero it.
  const resetUsage = useCallback(() => {
    usageEpoch.current += 1;
    setUsage(EMPTY_USAGE);
    setLatestUsage(null);
  }, []);

  const queueEvent = useCallback(
    (event: GuiWireEvent) => {
      eventsRef.current.push(event);
      if (!flushScheduled.current) {
        flushScheduled.current = true;
        requestAnimationFrame(() => flushEvents());
      }
    },
    [flushEvents],
  );

  // On mount: load the launch session (fresh empty) + the session list.
  useEffect(() => {
    void (async () => {
      const snapshot = await refreshSnapshot();
      if (snapshot) {
        newBlockSeq();
        setBlocks(blocksFromHistory(snapshot.messages));
        setLatestUsage(null);
      }
      await refreshSessions();
    })();
  }, [refreshSnapshot, refreshSessions]);

  async function send(text: string): Promise<void> {
    setRunning(true);
    setError(null);
    try {
      await api.prompt(text, queueEvent);
    } catch (e) {
      setRunning(false);
      setError(errorMessage(e));
    }
  }

  async function stop(): Promise<void> {
    try {
      await api.cancel();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  // A `turn_finished` / `error` event ends the running state (handled in
  // `flushEvents`); nothing further is needed here.


  async function openSettings(): Promise<void> {
    try {
      setSettings(await api.getSettings());
      setSettingsOpen(true);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function saveSettings(next: GuiSettings): Promise<void> {
    await api.setSettings(next);
    // The model shown reflects the current session; a settings change applies
    // at the next prompt, so refresh the snapshot metadata now.
    await refreshSnapshot();
  }

  async function newSession(): Promise<void> {
    setBusy(true);
    try {
      await api.newSession();
      resetUsage();
      const snapshot = await refreshSnapshot();
      if (snapshot) {
        newBlockSeq();
        setBlocks(blocksFromHistory(snapshot.messages));
      }
      await refreshSessions();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function resumeSession(id: string): Promise<void> {
    if (id === currentId || running) return;
    setBusy(true);
    try {
      await api.resumeSession(id);
      resetUsage();
      const snapshot = await refreshSnapshot();
      if (snapshot) {
        newBlockSeq();
        setBlocks(blocksFromHistory(snapshot.messages));
      }
      await refreshSessions();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function deleteSession(id: string): Promise<void> {
    const confirmed = window.confirm(
      "Delete this session? Its history will be removed permanently.",
    );
    if (!confirmed) return;
    setBusy(true);
    try {
      await api.deleteSession(id);
      await refreshSessions();
      // If the deleted session was current, the host opened a fresh one.
      if (id === currentId) {
        resetUsage();
        const snapshot = await refreshSnapshot();
        if (snapshot) {
          newBlockSeq();
          setBlocks(blocksFromHistory(snapshot.messages));
        }
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function compact(): Promise<void> {
    setRunning(true);
    try {
      await api.compactNow(queueEvent);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRunning(false);
    }
  }

  // `/help` lists the GUI's applicable Commands + Skills, read from the same
  // `complete("/")` IPC that feeds the popup (single source of truth).
  async function showHelp(): Promise<void> {
    try {
      setHelpItems(await api.complete("/"));
      setHelpOpen(true);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  // `/usage` recalibrates the HUD from a fresh host snapshot and rings it once
  // as feedback (spec D7): it no longer appends a transcript row, and it never
  // starts a model turn.
  async function showUsage(): Promise<void> {
    try {
      const snapshot = await api.getSession();
      // This reading is the freshest one available: fence off any snapshot
      // that was requested earlier and could land after it.
      usageEpoch.current += 1;
      setUsage(snapshot.usage);
      setUsagePulse((n) => n + 1);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  // `/exit` closes the window (ADR-0026 D3): the frontend command set fires the
  // GUI's own actions rather than starting a model turn.
  function exit(): void {
    void getCurrentWindow()
      .close()
      .catch((e) => setError(errorMessage(e)));
  }

  return (
    <div className="flex h-screen bg-slate-100 font-sans text-slate-800">
      <Sidebar
        sessions={sessions}
        currentId={currentId}
        onSelect={(id) => void resumeSession(id)}
        onNew={() => void newSession()}
        onDelete={(id) => void deleteSession(id)}
        onOpenSettings={() => void openSettings()}
        busy={busy || running}
      />
      <main className="flex min-w-0 flex-1 flex-col">
        <TopBar
          title={title}
          model={model}
          context={context}
          onCompact={() => void compact()}
          compacting={running}
          usage={usage}
          latest={latestUsage}
          pulse={usagePulse}
        />
        {warning ? (
          <div className="border-b border-amber-200 bg-amber-50 px-4 py-1.5 text-xs text-amber-800">
            {warning}
          </div>
        ) : null}
        {error ? (
          <div className="flex items-center justify-between border-b border-red-200 bg-red-50 px-4 py-1.5 text-xs text-red-700">
            <span>{error}</span>
            <button
              type="button"
              onClick={() => setError(null)}
              className="font-medium underline"
            >
              Dismiss
            </button>
          </div>
        ) : null}
        <Transcript blocks={blocks} running={running} />
        <InputBox
          running={running}
          onSend={(t) => void send(t)}
          onStop={() => void stop()}
          disabled={busy}
          onNewSession={() => void newSession()}
          onCompact={() => void compact()}
          onUsage={() => void showUsage()}
          onExit={exit}
          onHelp={() => void showHelp()}
        />
      </main>
      {helpOpen ? <HelpModal items={helpItems} onClose={() => setHelpOpen(false)} /> : null}
      {settingsOpen ? (
        <SettingsModal
          settings={settings}
          onSave={saveSettings}
          onClose={() => setSettingsOpen(false)}
        />
      ) : null}
    </div>
  );
}
