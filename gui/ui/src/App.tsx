import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, errorMessage } from "./api";
import {
  blocksFromHistory,
  newBlockSeq,
  reduceTranscript,
  usageNotice,
  type Block,
} from "./transcript";
import type {
  CompletionItem,
  ContextInfo,
  GuiSettings,
  GuiWireEvent,
  SessionInfo,
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

  const refreshSessions = useCallback(async () => {
    try {
      setSessions(await api.listSessions());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const refreshSnapshot = useCallback(async () => {
    try {
      const snapshot = await api.getSession();
      setCurrentId(snapshot.id);
      setTitle(snapshot.title);
      setModel(snapshot.model);
      setContext(snapshot.context);
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
    const last = pending[pending.length - 1];
    if (last && (last.kind === "message" || last.kind === "turn_finished")) {
      void refreshSnapshot();
      void refreshSessions();
    }
  }, [refreshSnapshot, refreshSessions]);

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

  // `/usage` reports the current session's cumulative token usage as a usage
  // line in the transcript (mirrors the TUI's `show_usage`), read fresh from
  // the host snapshot rather than starting a model turn.
  async function showUsage(): Promise<void> {
    try {
      const snapshot = await api.getSession();
      setBlocks((prev) => [...prev, usageNotice(snapshot.usage)]);
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
