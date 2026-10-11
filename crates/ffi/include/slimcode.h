/*
 * slimcode.h — the complete C ABI of the slimcode FFI module (ABI version 1).
 *
 * slimcode-ffi is a cdylib that drives one slimcode agent session per opaque
 * handle. It is the interface for non-Rust hosts (Node >= 26 with the built-in
 * `node:ffi`, Python via ctypes/cffi, C, …): the C side never calls back into
 * the host language, no host runtime is assumed, and every payload is a UTF-8
 * JSON string.
 *
 * ---------------------------------------------------------------------------
 * Quick start
 * ---------------------------------------------------------------------------
 *
 *   void *session = slimcode_session_create("{\"cwd\":\"/work\",\"persist\":true,"
 *                                           "\"base_url\":\"https://…/v1\","
 *                                           "\"model\":\"qwen3-max\","
 *                                           "\"api_key\":\"sk-…\"}");
 *   if (!session) { fprintf(stderr, "%s\n", slimcode_last_error()); exit(1); }
 *
 *   // A real-time host runs the turn on a worker thread and pumps the queue
 *   // on its UI thread:
 *   //   worker: code = slimcode_session_prompt(session, "list the files");
 *   //   UI:     while ((e = slimcode_session_next_event(session, -1)))
 *   //               { handle_json(e); slimcode_free_string(e); }
 *
 *   slimcode_session_dispose(session);
 *
 * ---------------------------------------------------------------------------
 * ABI version
 * ---------------------------------------------------------------------------
 *
 * slimcode_abi_version() returns the integer ABI version this library
 * implements. A binding should assert it equals the version the binding was
 * written against (currently 1) and refuse to run otherwise.
 *
 * The ABI only changes compatibly: new symbols (for example a future
 * callback-based `subscribe`) and new optional JSON fields are additive and do
 * NOT bump the version; the version is bumped only when an existing signature,
 * return code or ownership rule changes incompatibly.
 *
 * ---------------------------------------------------------------------------
 * Errors, return codes and ownership
 * ---------------------------------------------------------------------------
 *
 * A failing call returns NULL (pointer results) or -1 (int32_t results) and
 * stores a human-readable message in the calling thread's last error.
 * slimcode_last_error() returns that message, or NULL when the call that last
 * updated it on this thread succeeded. One internal case is deliberately loud
 * rather than silent: if an event could not be encoded for the queue (it
 * cannot happen for the event shapes this version defines), the turn's thread
 * records the reason there even though the call itself reports success.
 *
 * The rule, stated once: every FALLIBLE entry point clears the previous
 * message on entry and sets this one on failure. The entry points that never
 * touch it are slimcode_abi_version(), slimcode_session_abort(),
 * slimcode_session_dispose(), slimcode_last_error() and
 * slimcode_free_string(), so the pointer stays valid until the next call that
 * updates it on the same thread; copy the message if it must outlive that.
 * The message is owned by the library: do NOT pass it to
 * slimcode_free_string. The state is thread-local: a failure on one thread
 * never shows up on another, and a fresh thread starts with no error.
 *
 * Every string RETURNED by the library (session_id, title, usage_json,
 * messages_json, next_event, compact_now) is allocated by the library and must
 * be released with slimcode_free_string() exactly once;
 * slimcode_free_string(NULL) is a no-op. Every returned string is UTF-8: an
 * interior NUL byte cannot exist in a C string, so it is escaped as the two
 * characters `\0` (backslash, zero) instead of truncating the payload.
 *
 * Rust panics never cross this ABI: an internal panic surfaces as the same
 * NULL / -1 plus a last_error, never as a crash.
 *
 * A cancelled turn is NOT an error: slimcode_session_prompt returns 1 and
 * last_error stays empty.
 *
 * ---------------------------------------------------------------------------
 * Threading
 * ---------------------------------------------------------------------------
 *
 * A handle is process-global and may be used from any thread:
 *
 *  - create / session_id / title / usage_json / messages_json / prompt /
 *    compact_now / dispose: callable from any thread. prompt and compact_now
 *    are single-flight per handle (a second concurrent call returns -1).
 *  - abort: safe from any thread at any time, including while a prompt is
 *    running; it does not wait for the session's lock.
 *  - next_event: blocks the calling thread by design — the C side never calls
 *    back into the host language, so any host language can pump it.
 *
 * ---------------------------------------------------------------------------
 * Options JSON (slimcode_session_create)
 * ---------------------------------------------------------------------------
 *
 * One JSON object; unknown fields are an error (a mistyped `api_key` must not
 * silently fall back to config.toml):
 *
 *   {
 *     "cwd": "/path/to/project",   // tools + project resolution bind here
 *     "persist": true,             // false = in-memory, zero files (default true)
 *     "session_id": "slimcode-…",  // resume a persisted session (needs persist)
 *     "base_url": "http://…",      // provider endpoint override
 *     "model": "…",                // model override
 *     "api_key": "…",              // key override
 *     "cache": false,              // explicit-cache flag override
 *     "home": "/path/to/home"      // pin the slimcode home (sessions/skills/…)
 *   }
 *
 * Omitted fields fall through to the SDK's four-layer resolution (explicit
 * options > environment > config.toml > defaults). `persist` defaults to true,
 * exactly like the CLI; `session_id` together with `persist: false` is an
 * error. Provider / tools / skills injection is deliberately not exposed —
 * Rust trait objects and tool values cannot cross a C ABI; `base_url` is the
 * one external dependency a host can replace.
 *
 * ---------------------------------------------------------------------------
 * Events JSON (slimcode_session_next_event)
 * ---------------------------------------------------------------------------
 *
 * Every event is one flat JSON object with a "kind" field, ready for a host
 * `switch (event.kind)`:
 *
 *   {"kind":"turn","turn":1}
 *   {"kind":"reasoning","text":"…"}
 *   {"kind":"text","text":"…"}
 *   {"kind":"tool_start","tool_call_id":"…","name":"…","arguments":"{…}"}
 *   {"kind":"tool_result","tool_call_id":"…","name":"…","ok":true,"result":"…"}
 *   {"kind":"stop","reason":"completed"|"cancelled"}
 *   {"kind":"usage","usage":{"prompt_tokens":0,"completion_tokens":0,
 *                            "total_tokens":0,
 *                            "prompt_tokens_details":{"cached_tokens":0,
 *                                                     "cache_creation_input_tokens":0}}}
 *   // `prompt_tokens_details` is null, not a zeroed object, when the endpoint
 *   // reported no details.
 *   {"kind":"message","message":{"kind":"llm","…":"…"},"usage":null}
 *   {"kind":"turn_finished","stop_reason":"completed"|"cancelled"}
 *   {"kind":"compacted","before":0,"after":0}
 *
 * `message.message` uses the persisted session-log shape ({"kind":"llm",…} /
 * {"kind":"compact_summary",…}), so a host can share one decoder with the JSONL
 * session files.
 *
 * `usage` payloads are the SDK's own TokenUsage serialization: the details
 * object shown above becomes `null` — never a zeroed object — when the endpoint
 * reported no `prompt_tokens_details`, so read it defensively. A `message`
 * event's `usage` is `null` for the user prompt and for tool results, and the
 * request's usage for an assistant reply.
 *
 * Order for one text turn: the user `message` first (the prompt enters history
 * before the model is called), then `turn`, then the streamed `reasoning` /
 * `text` items, then the assistant `message` (with its usage) followed by the
 * `usage` item, then `stop`, and finally `turn_finished` — the authoritative
 * end of the turn, always last (a turn-end auto-compaction's `compacted` may
 * follow it). A turn that failed before reaching the provider has no `stop`
 * and no `turn_finished`; the prompt return code plus slimcode_last_error()
 * carry the failure.
 *
 * ---------------------------------------------------------------------------
 * Handle lifetime
 * ---------------------------------------------------------------------------
 *
 * One handle = one session. create returns a handle or NULL. The pointer is
 * opaque: never dereference it, never pass it to another process.
 * slimcode_session_dispose() disposes the SDK session and invalidates the
 * handle: threads blocked in next_event wake up with NULL, the queue's
 * unconsumed events are dropped, later calls on the pointer fail, and a second
 * dispose is a no-op — do not use the pointer afterwards. abort(NULL) and
 * dispose(NULL) are no-ops.
 *
 * Each handle's event queue is unbounded: drain it while a turn runs (or it
 * grows with every unconsumed event). ABI version 1 has no multiplexing across
 * handles: pump one thread per session (or poll next_event(handle, 0)).
 */

#ifndef SLIMCODE_H
#define SLIMCODE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * The ABI version implemented by this library (currently 1).
 * No preconditions, never fails.
 */
uint32_t slimcode_abi_version(void);

/*
 * Create a session from the options JSON described at the top of this file.
 *
 * Returns an opaque handle, or NULL on failure (malformed JSON, an unknown
 * option field, a missing API key, `session_id` with `persist: false`, …) with
 * the reason in slimcode_last_error().
 *
 * The handle is owned by the caller until slimcode_session_dispose().
 */
void* slimcode_session_create(const char* options_json);

/*
 * Run one whole agent turn for `text` on the calling thread (synchronous). The
 * turn's events go to this handle's queue — a real-time host calls prompt on a
 * worker thread and pumps slimcode_session_next_event() on its UI thread.
 *
 * Returns:
 *    0  Completed: the model produced a final answer; the turn's events
 *       (including one `message` event per history entry and a final
 *       {"kind":"turn_finished","stop_reason":"completed"}) are queued.
 *    1  Cancelled: abort / dispose interrupted the turn. Text streamed so far
 *       was delivered as display events but does not enter history;
 *       last_error stays empty (cancellation is not an error).
 *   -1  Error: empty / whitespace-only text, a turn (or compact_now) already
 *       running on this handle, a disposed session, or a provider / tool
 *       failure. See slimcode_last_error(); the session stays usable.
 *
 * Single-flight: a concurrent prompt (or compact_now) on the same handle
 * returns -1 immediately.
 */
int32_t slimcode_session_prompt(void* session, const char* text);

/*
 * Take the next queued event (one JSON object; release it with
 * slimcode_free_string) or block until one arrives.
 *
 * `timeout_ms` semantics:
 *   < 0  block indefinitely (the pump-loop form),
 *   = 0  return immediately (poll),
 *   > 0  wait at most that many milliseconds.
 *
 * Returns:
 *   - an event as one flat JSON object (see "Events JSON"); release it with
 *     slimcode_free_string;
 *   - NULL with an EMPTY last_error when the timeout expires, when a timeout
 *     of 0 finds the queue empty, or when the call was already blocked and the
 *     handle then got disposed — "nothing yet" / "the session ended", not a
 *     failure;
 *   - NULL with last_error SET when the handle is NULL, unknown or already
 *     disposed at call time.
 *
 * Those last two cases are both NULL and are told apart by last_error alone;
 * that is what a host pump loop branches on.
 *
 * Unbounded queue: events are never dropped or merged; take them while the
 * turn runs.
 */
char* slimcode_session_next_event(void* session, int32_t timeout_ms);

/*
 * Force one compaction of the history outside a turn (the `/compact` command).
 *
 * Returns {"before":N,"after":M} (release it with slimcode_free_string) and
 * queues a {"kind":"compacted","before":N,"after":M} event; NULL plus
 * last_error on failure (the history is left untouched).
 *
 * Single-flight with slimcode_session_prompt: calling it while a turn runs
 * returns NULL with "a turn is already running".
 */
char* slimcode_session_compact_now(void* session);

/*
 * Cancel the in-flight turn: callable from any thread, at any time, including
 * while prompt is blocked in a provider or tool call (it also terminates a
 * running `bash` child of the default tool set).
 *
 * Touches only the cancel token and never waits for the session lock, so it
 * returns promptly. A no-op (no error) when no turn is running, and for NULL /
 * unknown handles.
 */
void slimcode_session_abort(void* session);

/*
 * Dispose the session and invalidate the handle: blocked next_event callers
 * wake up with NULL, later calls on the pointer fail, and a second dispose is
 * a no-op. NULL / unknown handles are a silent no-op; like abort, dispose
 * never touches last_error (see "Errors, return codes and ownership" at the
 * top of this file).
 */
void slimcode_session_dispose(void* session);

/*
 * The session id (`slimcode-<unix>-<pid>-<n>`); release the string with
 * slimcode_free_string. NULL plus last_error for a NULL / disposed handle.
 */
char* slimcode_session_id(void* session);

/*
 * The inferred session title, or the EMPTY STRING (never NULL) for a session
 * that has no title yet; release the string with slimcode_free_string.
 */
char* slimcode_session_title(void* session);

/*
 * The session's accumulated token usage as a JSON object in the same shape as
 * the `usage` event payload; release the string with slimcode_free_string.
 * NULL plus last_error for a NULL / disposed handle.
 */
char* slimcode_session_usage_json(void* session);

/*
 * The session's message history as a JSON array of session-log-shaped objects
 * ({"kind":"llm",…} / {"kind":"compact_summary",…}); release the string with
 * slimcode_free_string. NULL plus last_error for a NULL / disposed handle.
 */
char* slimcode_session_messages_json(void* session);

/*
 * The calling thread's most recent error message, or NULL when the call that
 * last updated it on this thread succeeded.
 *
 * Library-owned: do NOT free this pointer. It stays valid until the next call
 * that updates it on the same thread (see "Errors, return codes and
 * ownership" at the top of this file), so copy it if it must outlive that.
 */
const char* slimcode_last_error(void);

/*
 * Release a string the library returned. NULL is a no-op. The pointer must
 * come from one of the functions above that documents slimcode_free_string as
 * the release path, and must be released exactly once; the pointer returned by
 * slimcode_last_error() must never be passed here.
 */
void slimcode_free_string(char* s);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SLIMCODE_H */