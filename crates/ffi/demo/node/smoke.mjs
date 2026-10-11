#!/usr/bin/env node
// Zero-dependency smoke test for the slimcode-ffi C ABI (ticket 01).
//
// Loads the cdylib the same way a real Node host would — the built-in
// `node:ffi` module (Node >= 26, no npm / koffi) — and exercises the ticket-01
// contract end to end: ABI version, options-JSON creation, the read-only
// accessors, the structured error path, handle disposal and string ownership.
//
// Ticket 07 grows this into the full demo (`demo.mjs`); `demo/run.sh` builds
// the library and passes its path in.
//
// Usage: node smoke.mjs <path-to-libslimcode_ffi>
//        SLIMCODE_FFI_LIB=<path> node smoke.mjs
//
// Prints PASS (exit 0) or FAIL (exit 1). The `node:ffi` ExperimentalWarning on
// stderr is not a failure.

import { dlopen, toString } from 'node:ffi';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const NULL = 0n;

const libraryPath = process.argv[2] ?? process.env.SLIMCODE_FFI_LIB;
if (!libraryPath) {
  console.error('FAIL: usage: node smoke.mjs <path-to-libslimcode_ffi>');
  process.exit(2);
}

// The ticket-01 symbols. A pointer result arrives as a bigint address (0n is
// NULL); a `string` argument is copied to a temporary NUL-terminated UTF-8
// buffer by node:ffi for the duration of the call.
const { functions } = dlopen(libraryPath, {
  slimcode_abi_version: { arguments: [], return: 'uint32' },
  slimcode_session_create: { arguments: ['string'], return: 'pointer' },
  slimcode_session_id: { arguments: ['pointer'], return: 'pointer' },
  slimcode_session_title: { arguments: ['pointer'], return: 'pointer' },
  slimcode_session_dispose: { arguments: ['pointer'], return: 'void' },
  slimcode_last_error: { arguments: [], return: 'pointer' },
  slimcode_free_string: { arguments: ['pointer'], return: 'void' },
});

/** Copy a library-owned string, then release it (the ownership rule). */
function takeString(pointer) {
  if (pointer === NULL) return null;
  const text = toString(pointer);
  functions.slimcode_free_string(pointer);
  return text;
}

/** Copy `last_error` — library-owned, so it must never reach free_string. */
function lastError() {
  const pointer = functions.slimcode_last_error();
  return pointer === NULL ? null : toString(pointer);
}

const results = [];
function check(name, ok, detail = '') {
  results.push({ name, ok });
  if (ok) {
    console.log(`  ok   ${name}`);
  } else {
    console.error(`  FAIL ${name}${detail ? ` — ${detail}` : ''}`);
  }
}

async function main() {
  const root = await mkdtemp(join(tmpdir(), 'slimcode-ffi-smoke-'));
  const home = join(root, 'home');
  const cwd = join(root, 'proj');
  await mkdir(home, { recursive: true });
  await mkdir(join(cwd, '.git'), { recursive: true });
  const options = {
    home,
    cwd,
    persist: false, // in-memory: the smoke test writes nothing
    base_url: 'http://127.0.0.1:1/v1',
    model: 'smoke-model',
    api_key: 'sk-smoke',
  };

  check('abi_version is 1', functions.slimcode_abi_version() === 1);

  // --- the error path: NULL handle + a structured message -------------------
  const broken = functions.slimcode_session_create('{');
  check('malformed options JSON returns NULL', broken === NULL, `got ${broken}`);
  const brokenReason = lastError();
  check(
    'last_error explains the malformed JSON',
    typeof brokenReason === 'string' && brokenReason.length > 0,
    brokenReason ?? '(empty)',
  );

  const typo = functions.slimcode_session_create(JSON.stringify({ api_ky: 'sk-typo' }));
  check('an unknown option field returns NULL', typo === NULL, `got ${typo}`);
  const typoReason = lastError();
  check(
    'last_error names the unknown field',
    typeof typoReason === 'string' && typoReason.includes('api_ky'),
    typoReason ?? '(empty)',
  );

  // --- the success path: create / read / dispose ---------------------------
  const session = functions.slimcode_session_create(JSON.stringify(options));
  check('create() returns a handle', session !== NULL && session > 0n, `got ${session}`);
  if (session === NULL) {
    console.error(`  (create failed: ${lastError() ?? 'no error message'})`);
    await rm(root, { recursive: true, force: true });
    return;
  }

  check('a successful call clears last_error', lastError() === null, lastError() ?? '');
  const id = takeString(functions.slimcode_session_id(session));
  check(
    'session_id is a slimcode id',
    typeof id === 'string' && id.startsWith('slimcode-'),
    String(id),
  );
  const title = takeString(functions.slimcode_session_title(session));
  check('a new session has an empty (not NULL) title', title === '', JSON.stringify(title));

  // Ownership primitive: NULL must be tolerated, and disposal is idempotent.
  functions.slimcode_free_string(NULL);
  functions.slimcode_session_dispose(session);
  functions.slimcode_session_dispose(session);
  check('dispose is silent and idempotent', lastError() === null, lastError() ?? '');

  await rm(root, { recursive: true, force: true });
}

await main();

const failed = results.filter((result) => !result.ok).length;
if (failed > 0) {
  console.error(`FAIL: slimcode-ffi smoke — ${failed}/${results.length} checks failed`);
  process.exitCode = 1;
} else {
  console.log(`PASS: slimcode-ffi smoke — ${results.length} checks`);
}