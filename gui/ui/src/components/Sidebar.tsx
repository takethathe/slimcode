import type { SessionInfo } from "../types";

// The left sidebar: the shared session repository list (title + time),
// a "new session" button and the settings entry (spec user stories 11-14,
// 17). Deleting is confirmed inline before it calls the host.

function formatTime(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000);
  const now = new Date();
  const sameDay = date.toDateString() === now.toDateString();
  return sameDay
    ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString([], { month: "short", day: "numeric" });
}

export function Sidebar({
  sessions,
  currentId,
  onSelect,
  onNew,
  onDelete,
  onOpenSettings,
  busy,
}: {
  sessions: SessionInfo[];
  currentId: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
  onOpenSettings: () => void;
  busy: boolean;
}) {
  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-slate-200 bg-slate-50">
      <div className="flex items-center gap-2 border-b border-slate-200 px-3 py-3">
        <button
          type="button"
          onClick={onNew}
          disabled={busy}
          className="flex-1 rounded-md bg-indigo-600 px-3 py-1.5 text-sm font-medium text-white shadow-sm hover:bg-indigo-500 disabled:opacity-50"
        >
          New session
        </button>
        <button
          type="button"
          onClick={onOpenSettings}
          title="Settings"
          className="rounded-md border border-slate-200 bg-white p-1.5 text-slate-500 hover:bg-slate-100"
        >
          <svg className="size-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.325.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 0 1 1.37.49l1.296 2.247a1.125 1.125 0 0 1-.26 1.431l-1.003.827c-.293.241-.438.613-.43.992a7.723 7.723 0 0 1 0 .255c-.008.378.137.75.43.991l1.004.827c.424.35.534.955.26 1.43l-1.298 2.247a1.125 1.125 0 0 1-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a6.47 6.47 0 0 1-.22.128c-.331.183-.581.495-.644.869l-.213 1.281c-.09.543-.56.94-1.11.94h-2.594c-.55 0-1.019-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a6.52 6.52 0 0 1-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 0 1-1.369-.49l-1.297-2.247a1.125 1.125 0 0 1 .26-1.431l1.004-.827c.292-.24.437-.613.43-.991a6.932 6.932 0 0 1 0-.255c.007-.38-.138-.751-.43-.992l-1.004-.827a1.125 1.125 0 0 1-.26-1.43l1.297-2.247a1.125 1.125 0 0 1 1.37-.491l1.216.456c.356.133.751.072 1.076-.124.072-.044.146-.086.22-.128.332-.183.582-.495.644-.869l.214-1.28Z"
            />
            <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" />
          </svg>
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-2 py-2">
        {sessions.length === 0 ? (
          <p className="px-2 py-6 text-center text-xs text-slate-400">
            No sessions yet.
          </p>
        ) : (
          <ul className="space-y-1">
            {sessions.map((session) => {
              const active = session.id === currentId;
              return (
                <li key={session.id}>
                  <div
                    className={`group flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm ${
                      active
                        ? "bg-indigo-50 text-indigo-800 ring-1 ring-inset ring-indigo-200"
                        : "text-slate-700 hover:bg-slate-100"
                    }`}
                    onClick={() => onSelect(session.id)}
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium">
                        {session.title || "Untitled session"}
                      </span>
                      <span className="block text-xs text-slate-400">
                        {formatTime(session.modified)}
                        {session.messages > 0
                          ? ` · ${session.messages} msg`
                          : ""}
                      </span>
                    </span>
                    <button
                      type="button"
                      title="Delete session"
                      disabled={busy}
                      onClick={(e) => {
                        e.stopPropagation();
                        onDelete(session.id);
                      }}
                      className="hidden rounded p-1 text-slate-400 hover:bg-red-50 hover:text-red-600 group-hover:block disabled:opacity-50"
                    >
                      <svg className="size-3.5" viewBox="0 0 20 20" fill="currentColor">
                        <path
                          fillRule="evenodd"
                          d="M8.75 1A2.75 2.75 0 0 0 6 3.75v.443c-.795.077-1.584.176-2.365.298a.75.75 0 1 0 .23 1.482l.149-.022.841 10.518A2.75 2.75 0 0 0 7.596 19h4.807a2.75 2.75 0 0 0 2.742-2.53l.841-10.52.149.023a.75.75 0 0 0 .23-1.482A41.03 41.03 0 0 0 14 4.193V3.75A2.75 2.75 0 0 0 11.25 1h-2.5ZM10 4c.84 0 1.673.025 2.5.075V3.75c0-.69-.56-1.25-1.25-1.25h-2.5c-.69 0-1.25.56-1.25 1.25v.325C8.327 4.025 9.16 4 10 4Z"
                          clipRule="evenodd"
                        />
                      </svg>
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </aside>
  );
}
