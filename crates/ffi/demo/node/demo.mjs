#!/usr/bin/env node
// Zero-dependency acceptance demo for the slimcode-ffi C ABI (ticket 07).
//
// One self-contained file, the built-in `node:ffi` module only (Node >= 26; no
// npm, no koffi), driven by `demo/run.sh` against the cdylib that runner
// builds. It is the executable counterpart of `include/slimcode.h`: every
// segment asserts the contract the header documents, against a local fake
// provider, fully offline and key-free.
//
// The threading shape a real host should copy:
//
//   * the turn runs on a worker thread (`slimcode_session_prompt` blocks its
//     caller) while this thread pumps `slimcode_session_next_event` — the
//     handle is one plain pointer value, usable from both threads;
//   * the provider is a local `node:http` SSE endpoint, so nothing leaves the
//     machine and no API key is needed;
//   * `abort` is called from this thread while the worker's turn is in flight;
//   * every session lives under an `mkdtemp` root, never the real
//     `~/.slimcode`, and the root is removed at the end.
//
// Prints one `PASS: …` and exits 0 only when every check passed; otherwise it
// prints the failures and exits non-zero.
//
// Usage: node demo.mjs <path-to-libslimcode_ffi>
//        SLIMCODE_FFI_LIB=<path> node demo.mjs

import { dlopen, toString } from 'node:ffi';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, readdir, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';

const NULL = 0n;
/** Re-import this file to get a worker that shares it (the `isMainThread` branch). */
const WORKER_URL = new URL(import.meta.url);
/** One `next_event` blocking slice; bounded so the worker's messages and the
 *  endpoint's timers keep running while this thread pumps. */
const PUMP_SLICE_MS = 100;
/** No turn in this demo may take longer; a hung demo is a failing demo. */
const TURN_TIMEOUT_MS = 30_000;

// The symbols this demo uses, declared exactly like `include/slimcode.h`
// (Node 26 wants `arguments` / `return`; `parameters` / `result` is silently
// ignored). Pointer results arrive as bigint addresses, 0n is NULL.
const LIBRARY_DECLARATIONS = {
  slimcode_abi_version: { arguments: [], return: 'uint32' },
  slimcode_session_create: { arguments: ['string'], return: 'pointer' },
  slimcode_session_prompt: { arguments: ['pointer', 'string'], return: 'int32' },
  slimcode_session_next_event: { arguments: ['pointer', 'int32'], return: 'pointer' },
  slimcode_session_abort: { arguments: ['pointer'], return: 'void' },
  slimcode_session_dispose: { arguments: ['pointer'], return: 'void' },
  slimcode_session_id: { arguments: ['pointer'], return: 'pointer' },
  slimcode_session_title: { arguments: ['pointer'], return: 'pointer' },
  slimcode_session_usage_json: { arguments: ['pointer'], return: 'pointer' },
  slimcode_session_messages_json: { arguments: ['pointer'], return: 'pointer' },
  slimcode_last_error: { arguments: [], return: 'pointer' },
  slimcode_free_string: { arguments: ['pointer'], return: 'void' },
};

// ---------------------------------------------------------------------------
// The ABI wrapper: raw symbols plus the header's ownership and error rules.
// ---------------------------------------------------------------------------

/**
 * Load the cdylib and wrap the symbols this demo uses. The wrappers apply the
 * two rules a host must not get wrong: every returned string is copied and then
 * released with `slimcode_free_string`, and `last_error` is copied but never
 * freed (it is library-owned, valid until the next call on this thread).
 */
function loadAbi(libraryPath) {
  const { functions } = dlopen(libraryPath, LIBRARY_DECLARATIONS);

  /** Copy a library-owned string and release it — the ABI's ownership rule. */
  const takeString = (pointer) => {
    if (pointer === NULL) return null;
    const text = toString(pointer);
    functions.slimcode_free_string(pointer);
    return text;
  };

  /** The calling thread's last_error; NULL means its last fallible call succeeded. */
  const lastError = () => {
    const pointer = functions.slimcode_last_error();
    return pointer === NULL ? null : toString(pointer);
  };

  /** Parse an accessor payload; NULL is a real error, so fail loudly. */
  const parseState = (symbol, pointer) => {
    const text = takeString(pointer);
    if (text === null) {
      throw new Error(`${symbol} returned NULL: ${lastError() ?? 'no error message'}`);
    }
    return JSON.parse(text);
  };

  return {
    raw: functions,
    lastError,
    takeString,
    abiVersion: () => functions.slimcode_abi_version(),
    create: (options) => functions.slimcode_session_create(JSON.stringify(options)),
    createJson: (optionsJson) => functions.slimcode_session_create(optionsJson),
    abort: (handle) => functions.slimcode_session_abort(handle),
    dispose: (handle) => functions.slimcode_session_dispose(handle),
    sessionId: (handle) => takeString(functions.slimcode_session_id(handle)),
    title: (handle) => takeString(functions.slimcode_session_title(handle)),
    usageJson: (handle) => parseState('usage_json', functions.slimcode_session_usage_json(handle)),
    messagesJson: (handle) =>
      parseState('messages_json', functions.slimcode_session_messages_json(handle)),

    /**
     * `next_event(handle, timeout_ms)` as a parsed event, or `null` for the
     * "nothing yet" return. The two NULL kinds are told apart by last_error
     * alone (the header's rule): an empty last_error is a timeout / an ended
     * session, a non-empty one is a real failure and is thrown here.
     */
    nextEvent(handle, timeoutMs) {
      const pointer = functions.slimcode_session_next_event(handle, timeoutMs);
      if (pointer === NULL) {
        const message = lastError();
        if (message !== null) throw new Error(`next_event failed: ${message}`);
        return null;
      }
      return JSON.parse(takeString(pointer));
    },
  };
}

// ---------------------------------------------------------------------------
// The fake Bailian compatible-mode provider: a local SSE endpoint.
// ---------------------------------------------------------------------------

/** The marker only the slow scenario streams; the abort segment keys on it. */
const PARTIAL_TEXT = 'partial-marker-7b21';
/** The `[DONE]` payload that terminates a streamed reply. */
const SSE_DONE = '[DONE]';

/** One streamed assistant text delta; `finishReason` closes the answer. */
function sseText(text, finishReason = null) {
  return JSON.stringify({
    id: 'c1',
    object: 'chat.completion.chunk',
    created: 1,
    model: 'mock',
    choices: [{ index: 0, delta: { content: text }, finish_reason: finishReason }],
  });
}

/** One reasoning (thinking) delta, the qwen-style `reasoning_content`. */
function sseReasoning(text) {
  return JSON.stringify({
    id: 'c1',
    object: 'chat.completion.chunk',
    created: 1,
    model: 'mock',
    choices: [{ index: 0, delta: { reasoning_content: text }, finish_reason: null }],
  });
}

/** One tool-call delta: id, tool name and the raw argument JSON string. */
function sseToolCall(id, name, argumentJson) {
  return JSON.stringify({
    id: 'c1',
    object: 'chat.completion.chunk',
    created: 1,
    model: 'mock',
    choices: [
      {
        index: 0,
        delta: {
          tool_calls: [
            { id, type: 'function', index: 0, function: { name, arguments: argumentJson } },
          ],
        },
        finish_reason: null,
      },
    ],
  });
}

/** The empty tail chunk that closes a tool-call reply with `finish_reason: "tool_calls"`. */
function sseToolFinish() {
  return JSON.stringify({
    id: 'c1',
    object: 'chat.completion.chunk',
    created: 1,
    model: 'mock',
    choices: [
      {
        index: 0,
        delta: { tool_calls: [{ index: 0, function: { name: null, arguments: '' } }] },
        finish_reason: 'tool_calls',
      },
    ],
  });
}

/**
 * The final usage-only chunk (`choices: []`), exactly how the real endpoint
 * reports token usage. `details` of `{ cached, cacheCreation }` includes
 * `prompt_tokens_details`; omitting it leaves that field `null` on the wire
 * (the shape `include/slimcode.h` pins, never a normalized zero object).
 */
function sseUsage(promptTokens, completionTokens, totalTokens, details = null) {
  const usage = {
    prompt_tokens: promptTokens,
    completion_tokens: completionTokens,
    total_tokens: totalTokens,
  };
  if (details) {
    usage.prompt_tokens_details = {
      cached_tokens: details.cached,
      cache_creation_input_tokens: details.cacheCreation,
    };
  }
  return JSON.stringify({
    id: 'c1',
    object: 'chat.completion.chunk',
    created: 1,
    model: 'mock',
    choices: [],
    usage,
  });
}

/**
 * Scenario name -> one scripted reply per request, served in request order.
 * `gapMs` drips the chunks slowly (so `abort` can land mid-stream); otherwise
 * the whole reply is written at once.
 */
const ENDPOINT_SCRIPTS = {
  // The tool turn: the model asks for `ls` first, then answers in a second
  // request that carries the tool result back to the model.
  tool: [
    { chunks: [sseToolCall('call_ls', 'ls', '{"path":"."}'), sseToolFinish(), SSE_DONE] },
    {
      chunks: [
        sseReasoning('Checking the listing.'),
        sseText('There it is.', 'stop'),
        sseUsage(20, 4, 24, { cached: 2, cacheCreation: 0 }),
        SSE_DONE,
      ],
    },
  ],
  // A deliberately slow drip: `abort` must interrupt it after the first chunk.
  slow: [
    {
      gapMs: 200,
      chunks: [
        sseText(PARTIAL_TEXT),
        sseText(' still streaming'),
        sseText(' and done', 'stop'),
        sseUsage(9, 2, 11),
        SSE_DONE,
      ],
    },
  ],
  // Plain text turns for the persistence segment.
  text: [
    { chunks: [sseText('Persisted.', 'stop'), sseUsage(10, 3, 13), SSE_DONE] },
    { chunks: [sseText('Appended.', 'stop'), sseUsage(20, 4, 24), SSE_DONE] },
  ],
};

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Start the fake endpoint on `127.0.0.1:0`. A session picks its script through
 * its `base_url`: `http://127.0.0.1:<port>/<scenario>/v1` makes the provider
 * post to `/<scenario>/v1/chat/completions`, so each session's requests are
 * answered from its own ordered script — and a scenario that runs out of
 * scripted replies fails loudly with a 500 instead of hanging the demo.
 */
async function startFakeEndpoint() {
  const requests = [];
  const server = createServer((req, res) => {
    // A client that aborts mid-stream (the cancel segment) destroys the
    // response; swallow its write error instead of crashing the demo.
    res.on('error', () => {});
    const body = [];
    req.on('data', (chunk) => body.push(chunk));
    req.on('end', () => {
      const [path] = (req.url ?? '').split('?');
      const record = {
        scenario: path.split('/')[1] ?? '',
        target: req.url,
        headers: req.headers,
        body: Buffer.concat(body).toString('utf8'),
      };
      requests.push(record);
      const script = ENDPOINT_SCRIPTS[record.scenario];
      const index = requests.filter((r) => r.scenario === record.scenario).length - 1;
      const reply = script?.[index];
      if (!reply) {
        res.writeHead(500, { 'content-type': 'text/plain; charset=utf-8' });
        res.end(`no scripted reply #${index + 1} for scenario "${record.scenario}"`);
        return;
      }
      streamReply(res, reply);
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  return {
    /** The `base_url` option that selects one scenario's script. */
    baseUrl: (scenario) => `http://127.0.0.1:${port}/${scenario}/v1`,
    requests,
    close() {
      server.closeAllConnections?.();
      server.close();
    },
  };
}

/** Write one scripted reply as `data: <payload>\n\n` chunks, then close. */
async function streamReply(res, reply) {
  res.writeHead(200, {
    'content-type': 'text/event-stream; charset=utf-8',
    'cache-control': 'no-cache',
    connection: 'close',
  });
  res.flushHeaders?.();
  for (const [index, payload] of reply.chunks.entries()) {
    if (index > 0 && reply.gapMs) {
      await sleep(reply.gapMs);
    }
    // The client may have aborted while we were dripping: stop writing, but
    // return so this handler always settles.
    if (res.destroyed || res.writableEnded) return;
    res.write(`data: ${payload}\n\n`);
  }
  res.end();
}

// ---------------------------------------------------------------------------
// The turn/worker split, the checks, and the segments.
// ---------------------------------------------------------------------------

/**
 * Run `slimcode_session_prompt` on a worker thread and pump this thread's event
 * queue while it runs. This is the demo's core claim: the queue is usable
 * across threads and the C side never calls back into JS — events arrive here
 * while the worker is still inside `prompt`.
 *
 * `onEvent(event, { workerDone })` runs on this thread as each event arrives
 * (that is where a host draws its UI and its abort / timeout logic). Every wait
 * is bounded: the C call uses a finite timeout, the loop has a deadline, and
 * the worker is terminated before returning.
 */
async function playTurn(abi, libraryPath, handle, text, { onEvent } = {}) {
  const worker = new Worker(WORKER_URL, { workerData: { libraryPath, handle, text } });
  const events = [];
  let result = null;
  let failure = null;
  let sawTurnFinished = false;
  let observedWhileRunning = false;
  let timedOut = false;
  const settled = new Promise((resolve) => {
    worker.on('message', (message) => {
      result = message;
      resolve();
    });
    worker.on('error', (error) => {
      failure = error;
      resolve();
    });
    worker.on('exit', (code) => {
      if (result === null && failure === null) {
        failure = new Error(`the prompt worker exited with code ${code} before reporting`);
        resolve();
      }
    });
  });

  const record = (event) => {
    if (result === null && failure === null) observedWhileRunning = true;
    events.push(event);
    if (event.kind === 'turn_finished') sawTurnFinished = true;
    onEvent?.(event, { workerDone: result !== null || failure !== null });
  };

  const deadline = Date.now() + TURN_TIMEOUT_MS;
  while (!sawTurnFinished) {
    const event = abi.nextEvent(handle, PUMP_SLICE_MS);
    if (event !== null) {
      record(event);
      continue;
    }
    if (result !== null || failure !== null) {
      // The worker's prompt returned, so every event this turn will ever queue
      // is queued by now (the FFI enqueues synchronously on the turn thread):
      // drain with non-blocking polls and stop. A missing turn_finished is then
      // a real gap, not a race with the worker.
      let pending;
      while ((pending = abi.nextEvent(handle, 0)) !== null) {
        record(pending);
      }
      break;
    }
    if (Date.now() > deadline) {
      timedOut = true;
      break;
    }
    // Hand this thread's event loop back: the worker's completion message and
    // the endpoint's next SSE chunk both need it (the C call above blocked it).
    await new Promise((resolve) => setImmediate(resolve));
  }

  if (timedOut) {
    failure = new Error(`the turn did not finish within ${TURN_TIMEOUT_MS}ms`);
  } else {
    await settled;
  }
  await worker.terminate(); // the turn is over (or timed out): never leave a worker behind
  if (failure !== null) throw failure;
  return {
    code: result.code,
    error: result.error,
    events,
    observedWhileRunning,
    sawTurnFinished,
  };
}

/** One event per line, as it arrives — the point of the pump. */
function printEvent(event) {
  let summary = event.kind;
  switch (event.kind) {
    case 'reasoning':
    case 'text':
      summary += ` ${JSON.stringify(event.text)}`;
      break;
    case 'tool_start':
      summary += ` ${event.name} ${event.tool_call_id} ${event.arguments}`;
      break;
    case 'tool_result':
      summary += ` ${event.name} ok=${event.ok}`;
      break;
    case 'stop':
      summary += ` ${event.reason}`;
      break;
    case 'turn_finished':
      summary += ` ${event.stop_reason}`;
      break;
    case 'message':
      summary += ` ${event.message.role}${event.usage ? ` usage=${event.usage.total_tokens}` : ''}`;
      break;
    case 'usage':
      summary += ` total=${event.usage.total_tokens}`;
      break;
    default:
      break;
  }
  console.log(`  event  ${summary}`);
}

const results = [];

/** Record one assertion; a failed check prints immediately and fails the run. */
function check(name, ok, detail = '') {
  results.push({ name, ok });
  if (ok) {
    console.log(`  ok   ${name}`);
  } else {
    console.error(`  FAIL ${name}${detail ? ` — ${detail}` : ''}`);
  }
}

/** Run one named segment, turning an unexpected throw into a failed check. */
async function runSegment(name, body) {
  console.log(`\n== ${name}`);
  try {
    await body();
  } catch (error) {
    check(`${name}: finished without an unexpected error`, false, error?.message ?? String(error));
  }
}

function report() {
  const failed = results.filter((result) => !result.ok).length;
  if (failed > 0) {
    console.error(`FAIL: slimcode-ffi Node demo — ${failed}/${results.length} checks failed`);
    process.exitCode = 1;
  } else {
    console.log(`PASS: slimcode-ffi Node demo — ${results.length} checks`);
  }
}

// ---------------------------------------------------------------------------
// The segments.
// ---------------------------------------------------------------------------

/**
 * The tool turn's event order, pinned by `crates/ffi/tests/tool_turn_accessors.rs`:
 * the user message, turn 1 with the assistant's tool call, the display pair,
 * the tool message, turn 2 with the answer, then usage / stop / turn_finished.
 */
const TOOL_TURN_KINDS = [
  'message',
  'turn',
  'message',
  'tool_start',
  'tool_result',
  'message',
  'turn',
  'reasoning',
  'text',
  'message',
  'usage',
  'stop',
  'turn_finished',
];

/** The text of one wire request message, whichever content shape the cache flag chose. */
function requestContent(message) {
  const { content } = message;
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) return content.map((block) => block.text ?? '').join('');
  return '';
}

/** The text of one `AgentMessage` payload (its concatenated `parts`). */
function messageText(message) {
  return (message.parts ?? []).map((part) => part.text ?? '').join('');
}

/**
 * Stable JSON (object keys sorted, recursively): comparing parsed data must not
 * depend on the serializer's key order.
 */
function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    const entries = Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`);
    return `{${entries.join(',')}}`;
  }
  return JSON.stringify(value);
}

const kinds = (events) => events.map((event) => event.kind);
const eventOfKind = (events, kind) => events.find((event) => event.kind === kind);
const textsOf = (events) => events.filter((event) => event.kind === 'text').map((event) => event.text);

/** Whether a path exists (the `persist: false` check: no store dir at all). */
async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

/** Every file under `dir`, recursively (any extension): "zero files" means zero. */
async function filesUnder(dir) {
  const found = [];
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return found; // a missing directory has no files
  }
  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      found.push(...(await filesUnder(path)));
    } else {
      found.push(path);
    }
  }
  return found.sort();
}

/** One append-only session log as parsed records (one JSON object per line). */
function logRecords(text) {
  return text
    .split('\n')
    .filter((line) => line.trim() !== '')
    .map((line) => JSON.parse(line));
}

async function main(libraryPath) {
  if (!libraryPath) {
    console.error('FAIL: usage: node demo.mjs <path-to-libslimcode_ffi>');
    process.exit(2);
  }
  const abi = loadAbi(libraryPath);
  const root = await mkdtemp(join(tmpdir(), 'slimcode-ffi-demo-'));
  const home = join(root, 'home');
  const cwd = join(root, 'proj');
  await mkdir(home, { recursive: true });
  await mkdir(join(cwd, '.git'), { recursive: true });
  const endpoint = await startFakeEndpoint();
  /**
   * The options every session uses: pinned `home` / `cwd` (never the real
   * `~/.slimcode`), a dummy key, and the local endpoint's scenario URL.
   */
  const options = (scenario, extra = {}) => ({
    home,
    cwd,
    base_url: endpoint.baseUrl(scenario),
    model: 'demo-model',
    api_key: 'sk-demo',
    ...extra,
  });

  try {
    check('abi_version is 1', abi.abiVersion() === 1, `got ${abi.abiVersion()}`);

    // -----------------------------------------------------------------------
    // Segment 1 — a tool turn: the worker runs `prompt`, this thread pumps.
    // -----------------------------------------------------------------------
    let sessionA = NULL;
    let toolTurn = null;
    await runSegment('tool turn: worker prompt + main-thread pump', async () => {
      sessionA = abi.create(options('tool', { persist: false }));
      check('create() returns a handle', sessionA !== NULL && sessionA > 0n, `got ${sessionA}`);
      if (sessionA === NULL) {
        throw new Error(`create failed: ${abi.lastError() ?? 'no error message'}`);
      }
      check('a successful create clears last_error', abi.lastError() === null);

      const turn = await playTurn(abi, libraryPath, sessionA, 'list the files', {
        onEvent: printEvent,
      });
      toolTurn = turn;
      check(
        "the worker's prompt returned Completed (0)",
        turn.code === 0,
        `code ${turn.code}${turn.error ? ` — ${turn.error}` : ''}`,
      );
      check(
        'events arrived while the worker was still in the turn (real-time pump)',
        turn.observedWhileRunning,
      );
      check(
        'the tool turn streamed the pinned event sequence',
        JSON.stringify(kinds(turn.events)) === JSON.stringify(TOOL_TURN_KINDS),
        JSON.stringify(kinds(turn.events)),
      );

      const start = eventOfKind(turn.events, 'tool_start');
      const tool = eventOfKind(turn.events, 'tool_result');
      check(
        'the tool_start / tool_result pair is the ls call and its real listing',
        start?.tool_call_id === 'call_ls' &&
          start?.name === 'ls' &&
          start?.arguments === '{"path":"."}' &&
          tool?.tool_call_id === 'call_ls' &&
          tool?.name === 'ls' &&
          tool?.ok === true &&
          (tool?.result ?? '').trim() === '.git',
        JSON.stringify({ start, tool }),
      );
      check(
        'stop and turn_finished report completed',
        eventOfKind(turn.events, 'stop')?.reason === 'completed' &&
          eventOfKind(turn.events, 'turn_finished')?.stop_reason === 'completed',
        JSON.stringify({ stop: eventOfKind(turn.events, 'stop'), finished: eventOfKind(turn.events, 'turn_finished') }),
      );

      // The tool round trip is only real if the endpoint saw the result come
      // back in the second request of the turn.
      const toolRequests = endpoint.requests.filter((request) => request.scenario === 'tool');
      const followUp = toolRequests[1] ? JSON.parse(toolRequests[1].body) : null;
      const toolMessage = followUp?.messages?.find((message) => message.role === 'tool');
      check(
        'the follow-up request carries the ls result back to the model',
        toolMessage?.tool_call_id === 'call_ls' && requestContent(toolMessage).trim() === '.git',
        JSON.stringify({ requests: toolRequests.length, toolMessage }),
      );
    });

    // -----------------------------------------------------------------------
    // Segment 2 — the read-only accessors describe the turn that just ran.
    // -----------------------------------------------------------------------
    await runSegment('accessors: session_id / title / usage_json / messages_json', async () => {
      if (toolTurn === null) throw new Error('the tool turn did not run');
      const id = abi.sessionId(sessionA);
      check(
        'session_id is a slimcode id',
        typeof id === 'string' && id.startsWith('slimcode-'),
        String(id),
      );
      const title = abi.title(sessionA);
      check('title is inferred from the first prompt', title === 'list the files', String(title));

      const usage = abi.usageJson(sessionA);
      check(
        'usage_json is the accumulated TokenUsage of the turn',
        canonical(usage) ===
          canonical({
            prompt_tokens: 20,
            completion_tokens: 4,
            total_tokens: 24,
            prompt_tokens_details: { cached_tokens: 2, cache_creation_input_tokens: 0 },
          }),
        JSON.stringify(usage),
      );

      // The accessor and the event queue describe the same history in the
      // session-log shape, so a host can parse all three (queue, accessor, JSONL
      // log) with one decoder.
      const history = abi.messagesJson(sessionA);
      const streamed = toolTurn.events
        .filter((event) => event.kind === 'message')
        .map((event) => event.message);
      check(
        'messages_json agrees with the message events, in order',
        canonical(history) === canonical(streamed),
        JSON.stringify({ history, streamed }),
      );
      check(
        'the history holds the prompt, the tool call, the tool result and the reply',
        Array.isArray(history) &&
          history.length === 4 &&
          history.map((message) => message.role).join(',') === 'user,assistant,tool,assistant' &&
          history[1].tool_calls?.[0]?.name === 'ls' &&
          history[1].tool_calls?.[0]?.arguments === '{"path":"."}' &&
          messageText(history[3]) === 'There it is.',
        JSON.stringify(history),
      );

      abi.dispose(sessionA);
      sessionA = NULL;
    });

    // -----------------------------------------------------------------------
    // Segment 3 — abort from this thread while the worker's turn streams.
    // -----------------------------------------------------------------------
    await runSegment('abort: cancel an in-flight turn from the main thread', async () => {
      const session = abi.create(options('slow', { persist: false }));
      check('create() returns a handle', session !== NULL && session > 0n, `got ${session}`);
      if (session === NULL) {
        throw new Error(`create failed: ${abi.lastError() ?? 'no error message'}`);
      }

      let abortLandedInFlight = false;
      let abortCalls = 0;
      const turn = await playTurn(abi, libraryPath, session, 'write a long answer', {
        onEvent: (event, { workerDone }) => {
          printEvent(event);
          if (abortCalls === 0 && event.kind === 'text' && event.text === PARTIAL_TEXT) {
            // The prompt is still inside the provider read on the worker, so
            // this is exactly the mid-stream Esc / timeout a host cancels.
            abortLandedInFlight = !workerDone;
            abortCalls += 1;
            abi.abort(session);
          }
        },
      });

      check(
        'the abort landed while the worker was still inside prompt',
        abortCalls === 1 && abortLandedInFlight,
        JSON.stringify({ abortCalls, abortLandedInFlight }),
      );
      check(
        'a cancelled turn returns Cancelled (1)',
        turn.code === 1,
        `code ${turn.code}${turn.error ? ` — ${turn.error}` : ''}`,
      );
      check(
        'cancellation is not an error on either thread',
        turn.error === null && abi.lastError() === null,
        JSON.stringify({ workerError: turn.error, mainError: abi.lastError() }),
      );
      check(
        'the partial text was delivered as a display event',
        textsOf(turn.events).includes(PARTIAL_TEXT),
        JSON.stringify(textsOf(turn.events)),
      );
      // The provider notices a cancel between socket reads (crates/ai/src/
      // provider.rs:202, ADR-0006 D6a: "reacts to an Esc cancel within one
      // socket chunk"), so the chunk that was
      // already in flight may still arrive — but the stream really was cut:
      // the script's remaining chunks never made it.
      check(
        'the abort cut the stream before the scripted final chunk',
        textsOf(turn.events).length <= 2 && !textsOf(turn.events).includes(' and done'),
        JSON.stringify(textsOf(turn.events)),
      );
      check(
        'stop and turn_finished report cancelled',
        turn.sawTurnFinished &&
          eventOfKind(turn.events, 'stop')?.reason === 'cancelled' &&
          eventOfKind(turn.events, 'turn_finished')?.stop_reason === 'cancelled',
        JSON.stringify({
          stop: eventOfKind(turn.events, 'stop'),
          finished: eventOfKind(turn.events, 'turn_finished'),
        }),
      );

      // The SDK appends a closing assistant message on a cancelled turn
      // (ADR-0009 D5; the string is crates/sdk/src/lib.rs:826 "The turn was
      // cancelled."), so the honest assertion is
      // the absence of the streamed partial text from the whole history, not an
      // empty one.
      const historyText = JSON.stringify(abi.messagesJson(session));
      check(
        'the streamed partial text did not enter history',
        !historyText.includes(PARTIAL_TEXT),
        historyText,
      );
      check(
        'the user prompt did enter history',
        historyText.includes('write a long answer'),
        historyText,
      );
      check(
        'the cancelled turn made exactly one request',
        endpoint.requests.filter((request) => request.scenario === 'slow').length === 1,
        JSON.stringify(endpoint.requests.map((request) => request.scenario)),
      );

      abi.dispose(session);
    });

    // -----------------------------------------------------------------------
    // Segment 4 — persistence: zero files in memory, a JSONL log on disk, and a
    // resume by `session_id` that replays it.
    // -----------------------------------------------------------------------
    await runSegment('persistence: persist: false vs a JSONL log and a session_id resume', async () => {
      // Segments 1-3 ran with `persist: false`: by now the temp home must hold
      // no file at all (not even a store directory).
      const inMemoryFiles = await filesUnder(home);
      check(
        'the persist: false sessions wrote zero files',
        inMemoryFiles.length === 0 && !(await exists(join(home, 'sessions'))),
        JSON.stringify(inMemoryFiles),
      );

      const first = abi.create(options('text', { persist: true }));
      if (first === NULL) {
        throw new Error(`create failed: ${abi.lastError() ?? 'no error message'}`);
      }
      const id = abi.sessionId(first);
      const firstTurn = await playTurn(abi, libraryPath, first, 'keep this turn', {
        onEvent: printEvent,
      });
      check(
        'the persisting turn completed',
        firstTurn.code === 0,
        `code ${firstTurn.code}${firstTurn.error ? ` — ${firstTurn.error}` : ''}`,
      );

      // The store's layout: <home>/sessions/<project-key>/<id>.jsonl.
      const sessionsRoot = join(home, 'sessions');
      const logs = (await filesUnder(sessionsRoot)).filter((path) => path.endsWith('.jsonl'));
      const parts = logs[0] ? logs[0].slice(sessionsRoot.length + 1).split('/') : [];
      check(
        'the log landed at <home>/sessions/<project-key>/<id>.jsonl',
        logs.length === 1 &&
          parts.length === 2 &&
          parts[0].startsWith('proj-') &&
          parts[1] === `${id}.jsonl`,
        JSON.stringify(logs),
      );
      if (logs.length !== 1) {
        throw new Error(`expected exactly one session log, got ${JSON.stringify(logs)}`);
      }

      // The log is the append-only shape `SessionStore::load` replays: a header,
      // a title record and one record per history entry, carrying the very
      // message payloads the ABI reported for the same turn.
      const records = logRecords(await readFile(logs[0], 'utf8'));
      const storedMessages = records.slice(2).map((record) => record.message);
      const streamedMessages = firstTurn.events
        .filter((event) => event.kind === 'message')
        .map((event) => event.message);
      check(
        'the log holds the header, the title and the turn messages',
        JSON.stringify(records.map((record) => record.type)) ===
          JSON.stringify(['session', 'title', 'message', 'message']) &&
          records[0].v === 1 &&
          records[0].id === id &&
          records[1].title === 'keep this turn' &&
          canonical(storedMessages) === canonical(streamedMessages),
        JSON.stringify(records),
      );

      // Keep the first handle's view, then resume it from disk through a second
      // handle carrying nothing but the same `session_id`.
      const firstMessages = abi.messagesJson(first);
      abi.dispose(first);

      const resumed = abi.create(options('text', { persist: true, session_id: id }));
      if (resumed === NULL) {
        throw new Error(`resume failed: ${abi.lastError() ?? 'no error message'}`);
      }
      check(
        'a resumed handle keeps its session id',
        abi.sessionId(resumed) === id,
        JSON.stringify({ id, resumed: abi.sessionId(resumed) }),
      );
      check(
        'the title is replayed from the log',
        abi.title(resumed) === 'keep this turn',
        JSON.stringify(abi.title(resumed)),
      );
      check(
        'the history is replayed from the log',
        canonical(abi.messagesJson(resumed)) === canonical(firstMessages),
        JSON.stringify({ replayed: abi.messagesJson(resumed), firstMessages }),
      );

      // A turn on the resumed handle must reach the endpoint with the replayed
      // conversation in its request body — the honest proof that `session_id`
      // really loaded the history (the endpoint is the only seam).
      const resumedTurn = await playTurn(abi, libraryPath, resumed, 'second question', {
        onEvent: printEvent,
      });
      check(
        'the resumed turn completed',
        resumedTurn.code === 0,
        `code ${resumedTurn.code}${resumedTurn.error ? ` — ${resumedTurn.error}` : ''}`,
      );
      const textRequests = endpoint.requests.filter((request) => request.scenario === 'text');
      const sentTexts = textRequests[1]
        ? JSON.parse(textRequests[1].body).messages.map(requestContent)
        : [];
      check(
        'the resumed request carries the replayed conversation',
        ['keep this turn', 'Persisted.', 'second question'].every((text) => sentTexts.includes(text)),
        JSON.stringify(sentTexts),
      );

      // Same file, appended to — never rewritten: one header, one title record,
      // and the new records are exactly the resumed turn's messages.
      const after = logRecords(await readFile(logs[0], 'utf8'));
      const appended = after.slice(records.length).map((record) => record.message);
      const resumedMessages = resumedTurn.events
        .filter((event) => event.kind === 'message')
        .map((event) => event.message);
      check(
        'the resumed turn appended to the same log without rewriting the header',
        after.length === records.length + 2 &&
          after.filter((record) => record.type === 'session').length === 1 &&
          after.filter((record) => record.type === 'title').length === 1 &&
          canonical(appended) === canonical(resumedMessages) &&
          appended.some((message) => messageText(message) === 'Appended.'),
        JSON.stringify(after),
      );
      check(
        'the resumed session still owns exactly one log',
        (await filesUnder(sessionsRoot)).filter((path) => path.endsWith('.jsonl')).length === 1,
        JSON.stringify(await filesUnder(sessionsRoot)),
      );

      abi.dispose(resumed);
    });

    // -----------------------------------------------------------------------
    // Segment 5 — the structured error path for bad options.
    // -----------------------------------------------------------------------
    await runSegment('bad options: NULL + last_error for malformed / unknown-field JSON', async () => {
      const malformed = abi.createJson('{');
      const malformedReason = abi.lastError();
      check('malformed options JSON returns NULL', malformed === NULL, `got ${malformed}`);
      check(
        'last_error explains the malformed JSON',
        typeof malformedReason === 'string' && malformedReason.length > 0,
        String(malformedReason),
      );

      // Unknown fields are an error on purpose: a mistyped `api_key` must fail
      // loudly instead of silently falling back to config.toml.
      const unknown = abi.createJson(JSON.stringify({ api_ky: 'sk-typo' }));
      const unknownReason = abi.lastError();
      check('an unknown option field returns NULL', unknown === NULL, `got ${unknown}`);
      check(
        'last_error names the unknown field',
        typeof unknownReason === 'string' && unknownReason.includes('api_ky'),
        String(unknownReason),
      );
    });
  } finally {
    endpoint.close();
    await rm(root, { recursive: true, force: true });
  }
  report();
}

/**
 * The worker side of the turn/worker split: run the blocking `prompt` here and
 * report the stop code. `last_error` is read in this thread because it is
 * thread-local — the main thread could never see this turn's failure.
 */
function runWorkerTurn() {
  const abi = loadAbi(workerData.libraryPath);
  const code = abi.raw.slimcode_session_prompt(workerData.handle, workerData.text);
  parentPort.postMessage({ code, error: abi.lastError() });
}

if (isMainThread) {
  await main(process.argv[2] ?? process.env.SLIMCODE_FFI_LIB);
} else {
  runWorkerTurn();
}