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
import { CardView } from "./components/CardView";
import { CARD_DEMO_OPS, cardSystemPrompt } from "./cardCatalog";
import {
  emptyCardsState,
  isGenerating,
  reduceCards,
  type CardEvent,
  type CardsState,
} from "./cards";

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

  // The Card view (spec .scratch/gui-cards, ADR-0030): its panel state comes
  // from the host's one-time Card subscription, batched per animation frame
  // like the transcript so a burst of ops is one React render. `cardPending`
  // starts at the click and ends at the first `card_end`, so the panel shows
  // feedback before the generator's kind line picks the target Card.
  const [cards, setCards] = useState<CardsState>(emptyCardsState);
  const [cardsCollapsed, setCardsCollapsed] = useState(false);
  const [cardNotice, setCardNotice] = useState<string | null>(null);
  const [cardPending, setCardPending] = useState(false);

  // The pending-turn event queue: events stream in via the Channel callback
  // (outside React), are reduced through a ref, and flushed into state on a
  // rAF so streaming stays smooth without re-rendering per fragment.
  const eventsRef = useRef<GuiWireEvent[]>([]);
  const flushScheduled = useRef(false);
  // The Card channel's queue: same pattern, separate rAF so a Card op never
  // waits on a transcript flush.
  const cardEventsRef = useRef<CardEvent[]>([]);
  const cardFlushScheduled = useRef(false);
  const previewTimer = useRef<number | null>(null);
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

  // A session swap empties the panel (ADR-0030 D2/D5): Cards describe one
  // session, so the previous session's conclusions must never read as this
  // one's. The host also pushes an empty snapshot; doing it locally keeps the
  // panel correct even if that push lands late.
  const resetCards = useCallback(() => {
    setCards(emptyCardsState());
    setCardNotice(null);
    setCardPending(false);
  }, []);

  // Manual generation is the retry / refresh path (D1). Single-flight is the
  // host's rule; the panel mirrors it so the user gets an immediate answer
  // instead of a silent no-op.
  async function generateCards(): Promise<void> {
    if (isGenerating(cards) || cardPending) {
      setCardNotice("Card generation is already running.");
      return;
    }
    setCardNotice(null);
    setCardPending(true);
    try {
      await api.regenerateCards(cardSystemPrompt());
    } catch (e) {
      setCardPending(false);
      const message = errorMessage(e);
      setCardNotice(
        message.includes("already running")
          ? "Card generation is already running."
          : message,
      );
    }
  }

  // Debug entry (dev builds only): replay a synthetic op stream into the
  // panel one line at a time, so the progressive growth is visible in a real
  // webview without spending a model call.
  function previewCards(): void {
    if (previewTimer.current !== null) window.clearTimeout(previewTimer.current);
    queueCardEvent({ kind: "card_begin", card_id: "result" });
    let index = 0;
    const step = (): void => {
      if (index >= CARD_DEMO_OPS.length) {
        queueCardEvent({ kind: "card_end", card_id: "result", ok: true });
        previewTimer.current = null;
        return;
      }
      queueCardEvent({
        kind: "card_op",
        card_id: "result",
        op: `${CARD_DEMO_OPS[index]}\n`,
      });
      index += 1;
      previewTimer.current = window.setTimeout(step, 120);
    };
    step();
  }

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

  const flushCardEvents = useCallback(() => {
    cardFlushScheduled.current = false;
    const pending = cardEventsRef.current;
    if (pending.length === 0) return;
    cardEventsRef.current = [];
    setCards((prev) => reduceCards(prev, pending));
    // A generation ended (this one or an automatic one): the pending hint is
    // stale either way. A begin clears the previous failure notice.
    if (pending.some((event) => event.kind === "card_end")) setCardPending(false);
    if (pending.some((event) => event.kind === "card_begin")) setCardNotice(null);
  }, []);

  const queueCardEvent = useCallback(
    (event: CardEvent) => {
      cardEventsRef.current.push(event);
      if (!cardFlushScheduled.current) {
        cardFlushScheduled.current = true;
        requestAnimationFrame(() => flushCardEvents());
      }
    },
    [flushCardEvents],
  );

  // Subscribe to the Card channel once (ADR-0030 D4). The command returns the
  // current snapshot, so a reloaded window restores the same Cards; every
  // later change arrives on the same callback. The system prompt is generated
  // from the catalog here and handed to the host, which keeps no second copy.
  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const snapshot = await api.subscribeCards(queueCardEvent, cardSystemPrompt());
        if (!active) return;
        setCards((prev) =>
          reduceCards(prev, [{ kind: "cards_snapshot", cards: snapshot }]),
        );
      } catch (e) {
        // A dead side channel is not a failed conversation (ADR-0030 D8).
        if (active) setCardNotice(errorMessage(e));
      }
    })();
    return () => {
      active = false;
    };
  }, [queueCardEvent]);

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
      resetCards();
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
      resetCards();
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
        resetCards();
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
        <div className="flex min-h-0 min-w-0 flex-1">
          <Transcript blocks={blocks} running={running} />
          <CardView
            cards={cards.cards}
            generating={isGenerating(cards) || cardPending}
            collapsed={cardsCollapsed}
            notice={cardNotice}
            onToggle={() => setCardsCollapsed((value) => !value)}
            onGenerate={() => void generateCards()}
            onPreview={previewCards}
          />
        </div>
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
