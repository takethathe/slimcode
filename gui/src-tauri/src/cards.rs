//! The host's Card view (ADR-0030 D2/D3/D4/D5/D7): the mirror of what the
//! webview renders, the tolerant SpecStream applier that keeps that mirror in
//! step, and the generator that asks a provider for the next Card.
//!
//! Two rules shape this module:
//!
//! - **One stream, two implementations.** The op payload crossing the wire is
//!   the provider's raw text fragment (it may be half a line). Both sides split
//!   on `\n`, keep the incomplete tail, and flush it when the generation ends,
//!   so the host's mirror and the webview's spec converge on the same value.
//!   The applier here reimplements `@json-render/core`'s patcher: writes to a
//!   missing target, auto-created parents, silent removal of a missing path.
//! - **Forward, then apply.** Every fragment reaches the webview before the
//!   host touches its own mirror: a host-side bug can lose the mirror but can
//!   never swallow a line the webview could have rendered.
//!
//! The generator runs on its own thread with its own provider and cancel
//! token: it never touches the session's history, provider, or usage, so a
//! Card can never bill the conversation (D5).

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::{Map, Value};

use slimcode_app::{AgentMessage, CancelToken, Delta, Message, Provider, ProviderConfig, Role};

/// The fixed Card kind vocabulary (ADR-0030 D2). A kind is a Card's identity:
/// generating the same kind again replaces that Card instead of appending.
pub const CARD_KINDS: [&str; 3] = ["task-board", "result", "insight"];

/// The kind a Card falls back to when the model's first line is not a kind
/// declaration (a malformed stream still deserves a visible, retryable Card).
const DEFAULT_KIND: &str = "result";

/// How many Cards one session keeps; beyond this the least recently generated
/// one is evicted (D2).
pub const MAX_CARDS: usize = 3;

/// Response cap for one Card generation (D7): a Card is a glance, not an essay.
const CARD_MAX_TOKENS: u32 = 4000;

/// History budget handed to the generator, in characters.
const TRANSCRIPT_MAX_CHARS: usize = 8_000;

/// How many trailing messages the digest keeps; what falls outside becomes a
/// one-line note (the model needs the current state, not the whole session).
const TRANSCRIPT_MAX_MESSAGES: usize = 20;

/// A tool result is clipped to this many characters: a result can be a whole
/// file, and a Card summarises state rather than replaying payloads.
const TOOL_RESULT_MAX_CHARS: usize = 400;

/// The sink the host pushes Card events into: the webview's `Channel` in
/// production, a recorder in tests.
pub type CardSink = Arc<dyn Fn(CardEvent) + Send + Sync>;

/// Builds the provider one generation drives (a fresh `BailianProvider` in
/// production, a scripted provider in tests).
pub type CardProviderFactory = Box<dyn FnOnce() -> Result<Box<dyn Provider + Send>, String> + Send>;

/// One Card as the webview restores it from a subscription's snapshot.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CardSnapshot {
    pub id: String,
    pub kind: String,
    pub spec: Value,
    /// Host-side recency counter; the snapshot array is most-recent-first.
    pub updated: u64,
}

/// The four flat events of the Card channel (D4). `card_op::op` is one raw
/// provider fragment — never re-serialized, never split differently than the
/// webview will split it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CardEvent {
    CardBegin { card_id: String },
    CardOp { card_id: String, op: String },
    CardEnd { card_id: String, ok: bool },
    CardsSnapshot { cards: Vec<CardSnapshot> },
}

// --- the tolerant SpecStream applier ---------------------------------------

/// Parse one JSON-pointer path into its unescaped segments (`""` → `[]`).
fn pointer_segments(path: &str) -> Vec<String> {
    if path.is_empty() {
        return Vec::new();
    }
    let raw = path.strip_prefix('/').unwrap_or(path);
    raw.split('/')
        .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
        .collect()
}

fn is_numeric_index(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit())
}

/// A dense array index: numeric and not past the end (`json-render`'s
/// `denseIndex`).
fn dense_index(len: usize, segment: &str) -> Option<usize> {
    if !is_numeric_index(segment) {
        return None;
    }
    let index: usize = segment.parse().ok()?;
    (index <= len).then_some(index)
}

fn is_container(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

/// The child a path segment creates when it is missing: an array when the next
/// segment indexes or appends, an object otherwise.
fn empty_container(next_is_numeric: bool) -> Value {
    if next_is_numeric {
        Value::Array(Vec::new())
    } else {
        Value::Object(Map::new())
    }
}

/// Walk the container path (every segment but the last), creating missing
/// intermediates like json-render does.
fn container_mut<'a>(root: &'a mut Value, segments: &[String]) -> Option<&'a mut Value> {
    let mut current = root;
    let parents = segments.len().saturating_sub(1);
    for position in 0..parents {
        let segment = &segments[position];
        let next_is_numeric = segments
            .get(position + 1)
            .is_some_and(|next| is_numeric_index(next) || next == "-");
        match current {
            Value::Array(items) => {
                let index = dense_index(items.len(), segment)?;
                if items.get(index).is_none_or(|item| !is_container(item)) {
                    let fresh = empty_container(next_is_numeric);
                    if index < items.len() {
                        items[index] = fresh;
                    } else {
                        items.push(fresh);
                    }
                }
                current = items.get_mut(index)?;
            }
            Value::Object(map) => {
                if map.get(segment).is_none_or(|item| !is_container(item)) {
                    map.insert(segment.clone(), empty_container(next_is_numeric));
                }
                current = map.get_mut(segment)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn add_at(current: &mut Value, segment: &str, value: Value) {
    match current {
        Value::Array(items) => {
            if segment == "-" {
                items.push(value);
            } else if let Some(index) = dense_index(items.len(), segment) {
                items.insert(index, value);
            }
        }
        Value::Object(map) => {
            map.insert(segment.to_string(), value);
        }
        _ => {}
    }
}

fn set_at(current: &mut Value, segment: &str, value: Value) {
    match current {
        Value::Array(items) => {
            if segment == "-" {
                items.push(value);
            } else if let Some(index) = dense_index(items.len(), segment) {
                // json-render assigns at `index <= len`, which appends on the
                // boundary; mirror that so a `replace` at the end cannot make
                // the host drop a line the webview accepted.
                if index < items.len() {
                    items[index] = value;
                } else {
                    items.push(value);
                }
            }
        }
        Value::Object(map) => {
            map.insert(segment.to_string(), value);
        }
        _ => {}
    }
}

fn remove_at(current: &mut Value, segment: &str) {
    match current {
        Value::Array(items) => {
            if segment == "-" {
                return;
            }
            if let Some(index) = dense_index(items.len(), segment)
                && index < items.len()
            {
                items.remove(index);
            }
        }
        Value::Object(map) => {
            map.remove(segment);
        }
        _ => {}
    }
}

fn get_at<'a>(current: &'a Value, segment: &str) -> Option<&'a Value> {
    match current {
        Value::Array(items) => segment
            .parse::<usize>()
            .ok()
            .and_then(|index| items.get(index)),
        Value::Object(map) => map.get(segment),
        _ => None,
    }
}

fn get_at_mut<'a>(current: &'a mut Value, segment: &str) -> Option<&'a mut Value> {
    match current {
        Value::Array(items) => segment
            .parse::<usize>()
            .ok()
            .and_then(|index| items.get_mut(index))
            .filter(|item| is_container(item)),
        Value::Object(map) => map.get_mut(segment).filter(|item| is_container(item)),
        _ => None,
    }
}

fn add_by_path(root: &mut Value, path: &str, value: Value) {
    let segments = pointer_segments(path);
    let Some((last, _)) = segments.split_last() else {
        return;
    };
    if segments.iter().any(|segment| segment == "__proto__") {
        return;
    }
    if let Some(container) = container_mut(root, &segments) {
        add_at(container, last, value);
    }
}

fn set_by_path(root: &mut Value, path: &str, value: Value) {
    let segments = pointer_segments(path);
    let Some((last, _)) = segments.split_last() else {
        return;
    };
    if segments.iter().any(|segment| segment == "__proto__") {
        return;
    }
    if let Some(container) = container_mut(root, &segments) {
        set_at(container, last, value);
    }
}

fn remove_by_path(root: &mut Value, path: &str) {
    let segments = pointer_segments(path);
    let Some((last, parents)) = segments.split_last() else {
        return;
    };
    if segments.iter().any(|segment| segment == "__proto__") {
        return;
    }
    // Removal never creates: a missing intermediate ends the walk silently.
    let mut current = root;
    for segment in parents {
        let Some(next) = get_at_mut(current, segment) else {
            return;
        };
        current = next;
    }
    remove_at(current, last);
}

fn get_by_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() || path == "/" {
        return Some(root);
    }
    let mut current = root;
    for segment in pointer_segments(path) {
        current = get_at(current, &segment)?;
    }
    Some(current)
}

fn deep_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| deep_equal(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| deep_equal(value, other)))
        }
        _ => left == right,
    }
}

/// Apply one SpecStream line to `spec`; `false` when the line is not a usable
/// patch (unparsable, an unknown op, a failed `test`) and was dropped.
pub fn apply_stream_line(spec: &mut Value, line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return false;
    }
    let Ok(patch) = serde_json::from_str::<Value>(trimmed) else {
        return false;
    };
    let Some(op) = patch.get("op").and_then(Value::as_str) else {
        return false;
    };
    let Some(path) = patch.get("path").and_then(Value::as_str) else {
        return false;
    };
    // An empty pointer addresses nothing: the webview drops the line, so the
    // host must not count it as applied either.
    if path.is_empty() {
        return false;
    }
    let value = || patch.get("value").cloned().unwrap_or(Value::Null);
    let from = || patch.get("from").and_then(Value::as_str);
    match op {
        "add" => add_by_path(spec, path, value()),
        "replace" => set_by_path(spec, path, value()),
        "remove" => remove_by_path(spec, path),
        "move" => {
            let Some(from) = from() else { return true };
            let moved = get_by_path(spec, from).cloned().unwrap_or(Value::Null);
            remove_by_path(spec, from);
            add_by_path(spec, path, moved);
        }
        "copy" => {
            let Some(from) = from() else { return true };
            let copied = get_by_path(spec, from).cloned().unwrap_or(Value::Null);
            add_by_path(spec, path, copied);
        }
        "test" => {
            let actual = get_by_path(spec, path).cloned().unwrap_or(Value::Null);
            return deep_equal(&actual, &value());
        }
        _ => return false,
    }
    true
}

// --- the per-Card mirror ---------------------------------------------------

/// The spec a Card starts from.
fn empty_spec() -> Value {
    let mut spec = Map::new();
    spec.insert("root".to_string(), Value::String(String::new()));
    spec.insert("elements".to_string(), Value::Object(Map::new()));
    Value::Object(spec)
}

/// One Card's mirror: its spec plus the incomplete trailing line between
/// fragments.
struct Mirror {
    kind: String,
    spec: Value,
    buffer: String,
    /// Lines applied since the current generation began.
    applied: u64,
    /// The spec shown before the current generation, restored on a dead end.
    previous: Option<Value>,
    failed: bool,
    updated: u64,
}

/// Every Card of one session, most recently generated first.
pub struct CardStore {
    cards: Vec<Mirror>,
    seq: u64,
}

impl Default for CardStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CardStore {
    pub fn new() -> Self {
        Self {
            cards: Vec::new(),
            seq: 0,
        }
    }

    /// Forget every Card (a session swap).
    pub fn clear(&mut self) {
        self.cards.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Start a generation for `kind`: replace that Card in place (keeping its
    /// spec as the fallback), or evict the oldest Card when the panel is full.
    pub fn begin(&mut self, kind: &str) {
        self.seq = self.seq.saturating_add(1);
        let previous = self
            .cards
            .iter()
            .find(|card| card.kind == kind)
            .map(|card| card.spec.clone());
        let mirror = Mirror {
            kind: kind.to_string(),
            spec: empty_spec(),
            buffer: String::new(),
            applied: 0,
            previous,
            failed: false,
            updated: self.seq,
        };
        self.cards.retain(|card| card.kind != kind);
        self.cards.insert(0, mirror);
        self.cards.truncate(MAX_CARDS);
    }

    /// Apply one raw fragment to `kind`'s mirror; `false` when no such Card is
    /// being generated (an op for a Card this session never began).
    pub fn apply(&mut self, kind: &str, fragment: &str) -> bool {
        let Some(card) = self.cards.iter_mut().find(|card| card.kind == kind) else {
            return false;
        };
        card.buffer.push_str(fragment);
        // Everything up to the last newline is complete; the rest stays
        // buffered for the next fragment (or the end flush).
        if let Some(cut) = card.buffer.rfind('\n') {
            let complete = card.buffer[..cut].to_string();
            card.buffer = card.buffer[cut + 1..].to_string();
            for line in complete.split('\n') {
                if apply_stream_line(&mut card.spec, line) {
                    card.applied += 1;
                }
            }
        }
        card.updated = self.seq;
        true
    }

    /// Finish a generation: flush the trailing fragment, restore the previous
    /// Card when nothing landed, and report whether the generation produced
    /// anything worth showing as a success.
    pub fn end(&mut self, kind: &str, request_ok: bool) -> bool {
        let Some(card) = self.cards.iter_mut().find(|card| card.kind == kind) else {
            return false;
        };
        let trailing = std::mem::take(&mut card.buffer);
        let trailing = trailing.trim().to_string();
        if !trailing.is_empty() && apply_stream_line(&mut card.spec, &trailing) {
            card.applied += 1;
        }
        let ok = request_ok && card.applied > 0;
        // D8 restores the previous Card only when *no* line landed: a stream
        // cancelled halfway keeps what the reader already sees, exactly as the
        // webview does.
        let previous = card.previous.take();
        if !ok
            && card.applied == 0
            && let Some(previous) = previous
        {
            card.spec = previous;
        }
        card.failed = !ok;
        card.updated = self.seq;
        ok
    }

    /// The current Cards, most recently generated first.
    pub fn snapshot(&self) -> Vec<CardSnapshot> {
        self.cards
            .iter()
            .map(|card| CardSnapshot {
                id: card.kind.clone(),
                kind: card.kind.clone(),
                spec: card.spec.clone(),
                updated: card.updated,
            })
            .collect()
    }
}

// --- the channel + generator ----------------------------------------------

/// The mutable Card state: the mirror, the subscriber, and the generation in
/// flight. Its own lock, so a Card never waits on the session lock.
struct CardRuntime {
    store: CardStore,
    sink: Option<CardSink>,
    /// Generated by the webview from the catalog; the host keeps no second
    /// copy of the component vocabulary.
    system_prompt: Option<String>,
    /// The generation in flight (token + thread), if any.
    active: Option<Generation>,
    /// Handles of finished or cancelled generations, joined by `dispose`.
    retired: Vec<JoinHandle<()>>,
}

struct Generation {
    token: CancelToken,
    handle: JoinHandle<()>,
}

/// The host's Card channel: the single source of truth for the panel (D3).
#[derive(Clone)]
pub struct CardController {
    inner: Arc<Mutex<CardRuntime>>,
}

impl Default for CardController {
    fn default() -> Self {
        Self::new()
    }
}

impl CardController {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CardRuntime {
                store: CardStore::new(),
                sink: None,
                system_prompt: None,
                active: None,
                retired: Vec::new(),
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CardRuntime> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The current snapshot (what a reloading window restores).
    pub fn snapshot(&self) -> Vec<CardSnapshot> {
        self.lock().store.snapshot()
    }

    /// Attach the webview's sink and the system prompt it generated. Returns
    /// the snapshot the subscription starts from.
    pub fn subscribe(&self, sink: CardSink, system_prompt: Option<String>) -> Vec<CardSnapshot> {
        let mut runtime = self.lock();
        runtime.sink = Some(sink);
        if system_prompt.is_some() {
            runtime.system_prompt = system_prompt;
        }
        runtime.store.snapshot()
    }

    /// Replace the system prompt the next generation uses (the webview
    /// regenerated it from its catalog).
    pub fn set_system_prompt(&self, system_prompt: String) {
        self.lock().system_prompt = Some(system_prompt);
    }

    /// Empty the panel and tell the webview (a session swap, D2/D5).
    pub fn reseed(&self) {
        let sink = {
            let mut runtime = self.lock();
            runtime.store.clear();
            runtime.sink.clone()
        };
        if let Some(sink) = sink {
            sink(CardEvent::CardsSnapshot { cards: Vec::new() });
        }
    }

    fn emit(&self, event: CardEvent) {
        let sink = self.lock().sink.clone();
        if let Some(sink) = sink {
            sink(event);
        }
    }

    /// Forward the begin, then start the mirror (D3: forward before applying).
    fn begin(&self, kind: &str) {
        self.emit(CardEvent::CardBegin {
            card_id: kind.to_string(),
        });
        self.lock().store.begin(kind);
    }

    /// Forward one raw fragment, then apply it to the mirror.
    fn forward(&self, kind: &str, fragment: &str) {
        self.emit(CardEvent::CardOp {
            card_id: kind.to_string(),
            op: fragment.to_string(),
        });
        self.lock().store.apply(kind, fragment);
    }

    /// Finish a generation: flush the trailing fragment, mark the Card, and
    /// release the single-flight slot.
    fn finish(&self, framing: &mut Framing, request_ok: bool) {
        let kind = match framing.kind.clone() {
            Some(kind) => kind,
            None if framing.token.is_cancelled() => {
                // Cancelled before the first line: no Card was opened, and
                // opening one now would paint the result of a generation the
                // user already abandoned (D4). Only clear the panel's hint.
                self.emit(CardEvent::CardEnd {
                    card_id: DEFAULT_KIND.to_string(),
                    ok: false,
                });
                return;
            }
            None => {
                // The request failed before its first line: still show the
                // fallback Card so the panel can offer a retry (D8).
                let kind = DEFAULT_KIND.to_string();
                self.begin(&kind);
                framing.kind = Some(kind.clone());
                kind
            }
        };
        if !framing.pending.is_empty() {
            let trailing = std::mem::take(&mut framing.pending);
            self.forward(&kind, &trailing);
        }
        let ok = self.lock().store.end(&kind, request_ok);
        self.emit(CardEvent::CardEnd { card_id: kind, ok });
    }

    /// Cancel the generation in flight without waiting for it (a main turn is
    /// starting, D4). A finished handle is joined immediately; a live one is
    /// retired and joined by `dispose`, so this never blocks the caller.
    pub fn cancel(&self) {
        let mut runtime = self.lock();
        let finished = runtime
            .active
            .as_ref()
            .is_some_and(|active| active.handle.is_finished());
        if finished {
            if let Some(active) = runtime.active.take() {
                let _ = active.handle.join();
            }
            return;
        }
        if let Some(active) = runtime.active.take() {
            active.token.cancel();
            runtime.retired.push(active.handle);
        }
    }

    /// Cancel and join every generation thread (host teardown).
    pub fn dispose(&self) {
        self.cancel();
        let handles = {
            let mut runtime = self.lock();
            std::mem::take(&mut runtime.retired)
        };
        for handle in handles {
            let _ = handle.join();
        }
    }

    /// Start one generation. Single-flight: a live generation rejects the
    /// request ("already running") instead of queueing (D7).
    pub fn generate(
        &self,
        factory: CardProviderFactory,
        config: ProviderConfig,
        transcript: String,
        trigger: CardTrigger,
    ) -> Result<(), String> {
        let (system_prompt, snapshot) = {
            let mut runtime = self.lock();
            let finished = runtime
                .active
                .as_ref()
                .is_some_and(|active| active.handle.is_finished());
            if finished && let Some(active) = runtime.active.take() {
                let _ = active.handle.join();
            }
            if runtime.active.is_some() {
                return Err("card generation already running".to_string());
            }
            let system_prompt = runtime
                .system_prompt
                .clone()
                .ok_or_else(|| "the Card channel has no subscriber".to_string())?;
            (system_prompt, runtime.store.snapshot())
        };
        let user_prompt = card_user_prompt(&snapshot, &transcript, trigger);
        // A one-off call: no tools, no cache writes, bounded response (D5/D7).
        let config = config.with_cache(false).with_max_tokens(CARD_MAX_TOKENS);
        let token = CancelToken::new();
        let cards = self.clone();
        let generation_token = token.clone();
        let handle = std::thread::spawn(move || {
            let mut framing = Framing::new(generation_token);
            let request = [
                Message::text(Role::System, system_prompt),
                Message::text(Role::User, user_prompt),
            ];
            let request_ok = match factory() {
                Ok(mut provider) => {
                    let token = framing.token.clone();
                    let result = provider.chat(&request, &[], &config, &token, &mut |delta| {
                        if let Delta::Text(fragment) = delta {
                            framing.feed(&cards, &fragment);
                        }
                        Ok(())
                    });
                    result.is_ok() && !token.is_cancelled()
                }
                Err(_) => false,
            };
            cards.finish(&mut framing, request_ok);
        });
        self.lock().active = Some(Generation { token, handle });
        Ok(())
    }
}

/// The framing state of one generation: decides the target kind from the
/// stream's first line, then forwards every fragment verbatim.
struct Framing {
    token: CancelToken,
    pending: String,
    kind: Option<String>,
}

impl Framing {
    fn new(token: CancelToken) -> Self {
        Self {
            token,
            pending: String::new(),
            kind: None,
        }
    }

    /// Feed one raw fragment. Until the first line is complete nothing is
    /// forwarded: the host must know the Card before it can open it.
    fn feed(&mut self, cards: &CardController, fragment: &str) {
        if let Some(kind) = self.kind.clone() {
            cards.forward(&kind, fragment);
            return;
        }
        self.pending.push_str(fragment);
        loop {
            let Some(cut) = self.pending.find('\n') else {
                return;
            };
            // Leading blank lines are not the kind line: skip them instead of
            // deciding the Card's kind from a stray newline.
            if self.pending[..cut].trim().is_empty() {
                self.pending = self.pending[cut + 1..].to_string();
                continue;
            }
            let first = self.pending[..=cut].to_string();
            let rest = self.pending[cut + 1..].to_string();
            self.pending.clear();
            let declared = declared_kind(&first);
            let kind = declared.unwrap_or(DEFAULT_KIND).to_string();
            cards.begin(&kind);
            self.kind = Some(kind.clone());
            // The kind line is channel control, not spec: only a first line
            // that is not a kind declaration at all is forwarded as a patch.
            if declared.is_none() && !is_kind_line(&first) {
                cards.forward(&kind, &first);
            }
            if !rest.is_empty() {
                cards.forward(&kind, &rest);
            }
            return;
        }
    }
}

/// The kind a line declares, when it is `{"op":"add","path":"/kind",...}` with
/// one of the known kinds.
fn declared_kind(line: &str) -> Option<&'static str> {
    let patch: Value = serde_json::from_str(line.trim()).ok()?;
    if patch.get("path").and_then(Value::as_str) != Some("/kind") {
        return None;
    }
    let value = patch.get("value").and_then(Value::as_str)?;
    CARD_KINDS.iter().find(|kind| **kind == value).copied()
}

/// Whether a line is a kind declaration at all (even one naming an unknown
/// kind), so it is never applied to the spec.
fn is_kind_line(line: &str) -> bool {
    serde_json::from_str::<Value>(line.trim())
        .ok()
        .is_some_and(|patch| patch.get("path").and_then(Value::as_str) == Some("/kind"))
}

/// Why a generation ran. The model sees the same conversation either way, and
/// the reason is the one thing that tells an automatic run apart from a manual
/// refresh of an unchanged turn (spec: the user prompt states it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardTrigger {
    /// The turn that just finished completed: the usual trigger.
    TurnCompleted,
    /// The user asked for a new Card (the panel's Generate / retry).
    Manual,
}

impl CardTrigger {
    fn describe(self) -> &'static str {
        match self {
            Self::TurnCompleted => "the turn above just completed",
            Self::Manual => "the user asked to generate the cards now",
        }
    }
}

/// The user message of one generation: the Cards that exist (so the model can
/// rebuild them under the same kind), the conversation digest, and why this
/// generation runs.
fn card_user_prompt(snapshot: &[CardSnapshot], transcript: &str, trigger: CardTrigger) -> String {
    let mut prompt = String::new();
    if snapshot.is_empty() {
        prompt.push_str("No cards exist yet for this session.\n");
    } else {
        prompt.push_str(
            "Existing cards, most recent first. Reuse a kind to update that card; a new kind adds one.\n",
        );
        for card in snapshot {
            prompt.push_str(&format!("[{}] {}\n", card.kind, card.spec));
        }
    }
    prompt.push_str("\nConversation (most recent last):\n");
    if transcript.trim().is_empty() {
        prompt.push_str("(the session has no messages yet)\n");
    } else {
        prompt.push_str(transcript.trim());
        prompt.push('\n');
    }
    prompt.push_str(&format!("\nTrigger: {}.\n", trigger.describe()));
    prompt
}

/// How a message reads in the digest: user and assistant text in full, a tool
/// result clipped, a tool call reduced to its name.
fn project_message(message: &AgentMessage) -> Option<String> {
    if message.is_compact_summary() {
        let text = message.text_content();
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        return Some(format!("summary: {}", clip(text, TOOL_RESULT_MAX_CHARS)));
    }
    let calls = message.tool_calls();
    let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
    let text = message.text_content();
    let text = text.trim();
    if text.is_empty() && names.is_empty() {
        return None;
    }
    let role = match message.role() {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        // The generator brings its own system prompt; the session's is noise.
        Role::System => return None,
    };
    let body = match message.role() {
        // A tool result can be a whole file: keep a glance, not the payload.
        Role::Tool => clip(text, TOOL_RESULT_MAX_CHARS),
        _ => text.to_string(),
    };
    let mut line = if body.is_empty() {
        role.to_string()
    } else {
        format!("{role}: {body}")
    };
    if !names.is_empty() {
        line.push_str(&format!(" (tools: {})", names.join(", ")));
    }
    Some(line)
}

/// Clip `text` to `max` characters, marking that it continues.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(max).collect();
    clipped.push('\u{2026}');
    clipped
}

/// A compact projection of the session history for the generator: the last
/// [`TRANSCRIPT_MAX_MESSAGES`] messages, older ones replaced by a one-line note
/// (spec: prompt data boundary), all of it capped at [`TRANSCRIPT_MAX_CHARS`].
pub fn transcript_digest(messages: &[AgentMessage]) -> String {
    let start = messages.len().saturating_sub(TRANSCRIPT_MAX_MESSAGES);
    let mut body: Vec<String> = Vec::new();
    let mut used = 0usize;
    for message in messages[start..].iter().rev() {
        let Some(line) = project_message(message) else {
            continue;
        };
        used += line.chars().count() + 2;
        body.push(line);
        if used >= TRANSCRIPT_MAX_CHARS {
            break;
        }
    }
    body.reverse();
    let mut lines: Vec<String> = Vec::with_capacity(body.len() + 1);
    if start > 0 {
        lines.push(format!("({start} earlier messages omitted)"));
    }
    lines.extend(body);
    let joined = lines.join("\n\n");
    let length = joined.chars().count();
    if length <= TRANSCRIPT_MAX_CHARS {
        return joined;
    }
    joined.chars().skip(length - TRANSCRIPT_MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use slimcode_app::{FinishReason, TokenUsage};

    fn line(patch: Value) -> String {
        format!("{patch}\n")
    }

    fn empty() -> Value {
        empty_spec()
    }

    /// A recording sink: the test's stand-in for the webview's channel.
    fn recorder() -> (CardSink, Arc<Mutex<Vec<CardEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        (
            Arc::new(move |event| sink.lock().expect("events").push(event)),
            events,
        )
    }

    fn snapshot_events(events: &Arc<Mutex<Vec<CardEvent>>>) -> Vec<CardEvent> {
        events.lock().expect("events").clone()
    }

    fn wait_until(predicate: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !predicate() {
            assert!(std::time::Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn config() -> ProviderConfig {
        ProviderConfig::new("key", "url", "model")
    }

    /// A provider that streams a fixed script of fragments once.
    #[derive(Clone)]
    struct ScriptProvider {
        fragments: Vec<String>,
        error: Option<String>,
    }

    impl Provider for ScriptProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[slimcode_app::ToolSpec],
            _config: &ProviderConfig,
            _cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            for fragment in &self.fragments {
                on_delta(Delta::Text(fragment.clone()))?;
            }
            match &self.error {
                Some(message) => Err(message.clone()),
                None => {
                    on_delta(Delta::Done(FinishReason::Stop))?;
                    Ok(None)
                }
            }
        }
    }

    fn scripted(provider: ScriptProvider) -> CardProviderFactory {
        Box::new(move || Ok(Box::new(provider.clone())))
    }

    // --- the applier -----------------------------------------------------

    #[test]
    fn apply_creates_missing_parents_and_writes_missing_targets() {
        let mut spec = empty();
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"add","path":"/elements/root","value":{"type":"Text"}}))
        ));
        assert_eq!(spec["elements"]["root"]["type"], serde_json::json!("Text"));

        // `replace` writes even where nothing existed (json-render's setByPath).
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"replace","path":"/root","value":"root"}))
        ));
        assert_eq!(spec["root"], serde_json::json!("root"));

        // A numeric next segment creates an array instead of an object.
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"add","path":"/elements/root/children/0","value":"a"}))
        ));
        assert_eq!(
            spec["elements"]["root"]["children"],
            serde_json::json!(["a"])
        );
    }

    #[test]
    fn add_appends_and_inserts_in_arrays() {
        let mut spec = serde_json::json!({"items": ["a", "c"]});
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"add","path":"/items/-","value":"d"}))
        ));
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"add","path":"/items/1","value":"b"}))
        ));
        assert_eq!(spec["items"], serde_json::json!(["a", "b", "c", "d"]));
    }

    #[test]
    fn remove_of_a_missing_path_is_silent() {
        let mut spec = empty();
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"remove","path":"/elements/nope/children/3"}))
        ));
        assert_eq!(spec, empty());
    }

    #[test]
    fn move_copy_and_test_follow_the_same_rules() {
        let mut spec = serde_json::json!({"a": {"text": "hi"}});
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"copy","from":"/a","path":"/b"}))
        ));
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"move","from":"/b","path":"/c"}))
        ));
        assert!(spec.get("b").is_none());
        assert_eq!(spec["c"]["text"], serde_json::json!("hi"));
        // A failing `test` is the one op that reports a dropped line.
        assert!(!apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"test","path":"/c/text","value":"bye"}))
        ));
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"test","path":"/c/text","value":"hi"}))
        ));
    }

    #[test]
    fn replace_at_the_end_of_an_array_appends() {
        // json-render assigns at `index <= len`; on the boundary that appends.
        let mut spec = serde_json::json!({"items": ["a"]});
        assert!(apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"replace","path":"/items/1","value":"b"}))
        ));
        assert_eq!(spec["items"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn junk_lines_are_dropped() {
        let mut spec = empty();
        assert!(!apply_stream_line(&mut spec, "not json"));
        assert!(!apply_stream_line(&mut spec, "{\"op\":\"add\",\"path\":"));
        assert!(!apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"path":"/root"}))
        ));
        // An op json-render does not know, and an empty pointer: the webview
        // drops both, so the host must not count them as applied either.
        assert!(!apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"merge","path":"/root","value":"r"}))
        ));
        assert!(!apply_stream_line(
            &mut spec,
            &line(serde_json::json!({"op":"add","path":"","value":"r"}))
        ));
        assert_eq!(spec, empty());
    }

    #[test]
    fn the_shared_fixture_converges() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ui/src/fixtures/card-stream.json");
        let fixture: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json");
        let stream = fixture["stream"].as_array().expect("stream");
        let expected = fixture["expected"].as_object().expect("expected");

        let mut store = CardStore::new();
        for entry in stream {
            let card = entry["card"].as_str().expect("card");
            let op = entry["op"].as_str().expect("op");
            if !store
                .snapshot()
                .iter()
                .any(|snapshot| snapshot.kind == card)
            {
                store.begin(card);
            }
            assert!(store.apply(card, op), "apply {card}");
        }
        for kind in expected.keys() {
            assert!(store.end(kind, true), "end {kind}");
        }

        let snapshot = store.snapshot();
        for (kind, spec) in expected {
            let card = snapshot
                .iter()
                .find(|card| &card.kind == kind)
                .unwrap_or_else(|| panic!("missing card {kind}"));
            assert_eq!(&card.spec, spec, "spec of {kind}");
        }
        // Most recently begun first.
        assert_eq!(
            snapshot
                .iter()
                .map(|card| card.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["result", "task-board"]
        );
    }

    // --- the mirror's lifecycle -----------------------------------------

    #[test]
    fn a_split_line_waits_for_its_second_fragment() {
        let mut store = CardStore::new();
        store.begin("result");
        store.apply("result", "{\"op\":\"add\",\"path\":\"/root\"");
        assert_eq!(store.snapshot()[0].spec, empty());
        store.apply("result", ",\"value\":\"r\"}\n");
        assert_eq!(store.snapshot()[0].spec["root"], serde_json::json!("r"));
    }

    #[test]
    fn the_end_flushes_the_trailing_fragment() {
        let mut store = CardStore::new();
        store.begin("result");
        store.apply(
            "result",
            "{\"op\":\"add\",\"path\":\"/root\",\"value\":\"r\"}",
        );
        assert_eq!(store.snapshot()[0].spec["root"], serde_json::json!(""));
        assert!(store.end("result", true));
        assert_eq!(store.snapshot()[0].spec["root"], serde_json::json!("r"));
    }

    #[test]
    fn the_same_kind_replaces_and_reorders() {
        let mut store = CardStore::new();
        store.begin("result");
        store.apply(
            "result",
            &line(serde_json::json!({"op":"add","path":"/root","value":"r"})),
        );
        store.end("result", true);
        store.begin("insight");
        store.end("insight", false);
        assert_eq!(
            store
                .snapshot()
                .iter()
                .map(|c| c.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["insight", "result"]
        );

        store.begin("result");
        store.end("result", true);
        assert_eq!(
            store
                .snapshot()
                .iter()
                .map(|c| c.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["result", "insight"]
        );
    }

    #[test]
    fn at_most_three_cards_are_kept() {
        let mut store = CardStore::new();
        for kind in ["task-board", "result", "insight"] {
            store.begin(kind);
            store.end(kind, false);
        }
        store.begin("task-board");
        store.end("task-board", false);
        assert_eq!(store.snapshot().len(), MAX_CARDS);
        assert!(
            store.snapshot().iter().any(|card| card.kind == "result"),
            "eviction takes the least recently generated card"
        );
    }

    #[test]
    fn a_generation_that_applied_nothing_restores_the_previous_card() {
        let mut store = CardStore::new();
        store.begin("result");
        store.apply(
            "result",
            &line(serde_json::json!({"op":"add","path":"/root","value":"r"})),
        );
        assert!(store.end("result", true));

        store.begin("result");
        store.apply("result", "{\"op\":\"add\",\"path\":\n");
        assert!(!store.end("result", false));
        assert_eq!(store.snapshot()[0].spec["root"], serde_json::json!("r"));
    }

    #[test]
    fn a_half_stream_keeps_what_the_reader_already_saw() {
        let mut store = CardStore::new();
        store.begin("result");
        store.apply(
            "result",
            &line(serde_json::json!({"op":"add","path":"/root","value":"r"})),
        );
        assert!(store.end("result", true));

        // The next turn cancels this generation halfway through: one line
        // landed, so the panel and the mirror both keep it (D8 restores only
        // when nothing applied).
        store.begin("result");
        store.apply(
            "result",
            &line(serde_json::json!({"op":"add","path":"/root","value":"new"})),
        );
        assert!(!store.end("result", false));
        assert_eq!(store.snapshot()[0].spec["root"], serde_json::json!("new"));
    }

    // --- the generator ---------------------------------------------------

    #[test]
    fn a_generation_streams_begin_ops_then_end() {
        let cards = CardController::new();
        let (sink, events) = recorder();
        assert!(cards.subscribe(sink, Some("system".to_string())).is_empty());

        cards
            .generate(
                scripted(ScriptProvider {
                    fragments: vec![
                        "{\"op\":\"add\",\"path\":\"/kind\",\"value\":\"insight\"}\n".to_string(),
                        "{\"op\":\"add\",\"path\":\"/root\",\"value\":\"root\"}\n".to_string(),
                        "{\"op\":\"add\",\"path\":\"/elements/root\",\"value\":{\"type\":\"Text\"}}\n"
                            .to_string(),
                    ],
                    error: None,
                }),
                config(),
                "user: hi".to_string(),
                CardTrigger::TurnCompleted,
            )
            .expect("start");

        wait_until(|| !snapshot_events(&events).is_empty() && cards.snapshot().len() == 1);
        wait_until(|| {
            snapshot_events(&events)
                .iter()
                .any(|event| matches!(event, CardEvent::CardEnd { .. }))
        });

        let events = snapshot_events(&events);
        assert_eq!(
            events[0],
            CardEvent::CardBegin {
                card_id: "insight".to_string()
            }
        );
        assert!(matches!(&events[1], CardEvent::CardOp { card_id, .. } if card_id == "insight"));
        assert_eq!(
            events.last(),
            Some(&CardEvent::CardEnd {
                card_id: "insight".to_string(),
                ok: true
            })
        );
        let snapshot = cards.snapshot();
        assert_eq!(snapshot[0].kind, "insight");
        assert_eq!(snapshot[0].spec["root"], serde_json::json!("root"));
        assert_eq!(
            snapshot[0].spec["elements"]["root"]["type"],
            serde_json::json!("Text")
        );
    }

    #[test]
    fn leading_blank_lines_do_not_decide_the_kind() {
        let cards = CardController::new();
        let (sink, events) = recorder();
        cards.subscribe(sink, Some("system".to_string()));
        cards
            .generate(
                scripted(ScriptProvider {
                    fragments: vec![
                        "\n\n".to_string(),
                        "{\"op\":\"add\",\"path\":\"/kind\",\"value\":\"task-board\"}\n"
                            .to_string(),
                        "{\"op\":\"add\",\"path\":\"/root\",\"value\":\"root\"}\n".to_string(),
                    ],
                    error: None,
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("start");
        wait_until(|| {
            snapshot_events(&events)
                .iter()
                .any(|event| matches!(event, CardEvent::CardEnd { .. }))
        });
        let events = snapshot_events(&events);
        assert_eq!(
            events[0],
            CardEvent::CardBegin {
                card_id: "task-board".to_string()
            }
        );
        let snapshot = cards.snapshot();
        assert_eq!(snapshot[0].kind, "task-board");
        // The kind line never becomes spec content.
        assert!(snapshot[0].spec.get("kind").is_none());
    }

    #[test]
    fn a_malformed_first_line_falls_back_to_the_result_card() {
        let cards = CardController::new();
        let (sink, events) = recorder();
        cards.subscribe(sink, Some("system".to_string()));
        cards
            .generate(
                scripted(ScriptProvider {
                    fragments: vec![
                        "{\"op\":\"add\",\"path\":\"/root\",\"value\":\"root\"}\n".to_string(),
                    ],
                    error: None,
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("start");
        wait_until(|| {
            snapshot_events(&events)
                .iter()
                .any(|event| matches!(event, CardEvent::CardEnd { .. }))
        });
        let events = snapshot_events(&events);
        assert_eq!(
            events[0],
            CardEvent::CardBegin {
                card_id: "result".to_string()
            }
        );
        // The first line was a real patch, so it is forwarded too.
        assert!(matches!(&events[1], CardEvent::CardOp { card_id, .. } if card_id == "result"));
        assert_eq!(cards.snapshot()[0].spec["root"], serde_json::json!("root"));
    }

    #[test]
    fn a_failed_generation_ends_unok_and_the_retry_is_accepted() {
        let cards = CardController::new();
        let (sink, events) = recorder();
        cards.subscribe(sink, Some("system".to_string()));
        cards
            .generate(
                scripted(ScriptProvider {
                    fragments: vec![],
                    error: Some("boom".to_string()),
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("start");
        wait_until(|| {
            snapshot_events(&events)
                .iter()
                .any(|event| matches!(event, CardEvent::CardEnd { .. }))
        });
        let events = snapshot_events(&events);
        assert_eq!(
            events.last(),
            Some(&CardEvent::CardEnd {
                card_id: "result".to_string(),
                ok: false
            })
        );
        // Failure before any line still yields a Card to retry from.
        assert_eq!(cards.snapshot().len(), 1);

        cards
            .generate(
                scripted(ScriptProvider {
                    fragments: vec![
                        "{\"op\":\"add\",\"path\":\"/root\",\"value\":\"r\"}\n".to_string(),
                    ],
                    error: None,
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("retry accepted");
    }

    #[test]
    fn a_live_generation_rejects_a_second_request() {
        let cards = CardController::new();
        let (sink, _events) = recorder();
        cards.subscribe(sink, Some("system".to_string()));
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let started_provider = Arc::clone(&started);
        let release_provider = Arc::clone(&release);
        cards
            .generate(
                Box::new(move || {
                    Ok(Box::new(GatedProvider {
                        started: started_provider,
                        release: release_provider,
                    }))
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("start");
        wait_until(|| started.load(Ordering::SeqCst));

        let error = cards
            .generate(
                scripted(ScriptProvider {
                    fragments: vec![],
                    error: None,
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect_err("second request");
        assert!(error.contains("already running"), "{error}");

        release.store(true, Ordering::SeqCst);
        cards.dispose();
    }

    #[test]
    fn cancel_ends_the_generation_without_applying_a_card() {
        let cards = CardController::new();
        let (sink, events) = recorder();
        cards.subscribe(sink, Some("system".to_string()));
        let started = Arc::new(AtomicBool::new(false));
        let started_provider = Arc::clone(&started);
        cards
            .generate(
                Box::new(move || {
                    Ok(Box::new(CancelAwareProvider {
                        started: started_provider,
                    }))
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("start");
        wait_until(|| started.load(Ordering::SeqCst));

        cards.cancel();
        cards.dispose();
        let events = snapshot_events(&events);
        assert!(
            matches!(events.last(), Some(CardEvent::CardEnd { ok: false, .. })),
            "{:?}",
            events
        );
        // Cancelled before its first line: no Card was opened at all, so a
        // new turn cannot leave a phantom behind.
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, CardEvent::CardBegin { .. })),
            "{:?}",
            events
        );
        assert!(cards.snapshot().is_empty());
    }

    #[test]
    fn subscribing_restores_the_current_snapshot() {
        let cards = CardController::new();
        let (sink, _events) = recorder();
        cards.subscribe(sink, Some("system".to_string()));
        cards
            .generate(
                scripted(ScriptProvider {
                    fragments: vec![
                        "{\"op\":\"add\",\"path\":\"/kind\",\"value\":\"result\"}\n".to_string(),
                        "{\"op\":\"add\",\"path\":\"/root\",\"value\":\"r\"}\n".to_string(),
                    ],
                    error: None,
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("start");
        wait_until(|| !cards.snapshot().is_empty());
        wait_until(|| {
            !cards
                .snapshot()
                .iter()
                .any(|card| card.spec["root"] == serde_json::json!(""))
        });

        let (sink, _events) = recorder();
        let restored = cards.subscribe(sink, None);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].kind, "result");
        assert_eq!(restored[0].spec["root"], serde_json::json!("r"));
    }

    #[test]
    fn reseeding_empties_the_panel_and_the_snapshot() {
        let cards = CardController::new();
        let (sink, events) = recorder();
        cards.subscribe(sink, Some("system".to_string()));
        cards
            .generate(
                scripted(ScriptProvider {
                    fragments: vec![
                        "{\"op\":\"add\",\"path\":\"/root\",\"value\":\"r\"}\n".to_string(),
                    ],
                    error: None,
                }),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("start");
        wait_until(|| !cards.snapshot().is_empty());

        cards.reseed();
        assert!(cards.snapshot().is_empty());
        assert_eq!(
            snapshot_events(&events).last(),
            Some(&CardEvent::CardsSnapshot { cards: Vec::new() })
        );
    }

    /// What `ConfigProbe` observed per call: the token ceiling, the cache
    /// flag, and the number of tools it was offered.
    type ProbedConfig = (Option<u32>, bool, usize);

    /// A provider that parks inside `chat` until released: it hands back the
    /// configuration it was called with, so a test can pin the bounded,
    /// tool-less, uncached shape of a Card request (D5/D7).
    #[derive(Clone)]
    struct ConfigProbe {
        fragments: Vec<String>,
        seen: Arc<Mutex<Vec<ProbedConfig>>>,
    }

    impl Provider for ConfigProbe {
        fn chat(
            &mut self,
            _messages: &[Message],
            tools: &[slimcode_app::ToolSpec],
            config: &ProviderConfig,
            _cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.seen
                .lock()
                .expect("seen")
                .push((config.max_tokens, config.cache, tools.len()));
            for fragment in &self.fragments {
                on_delta(Delta::Text(fragment.clone()))?;
            }
            Ok(None)
        }
    }

    #[test]
    fn a_card_request_is_bounded_toolless_and_uncached() {
        let cards = CardController::new();
        let (sink, events) = recorder();
        cards.subscribe(sink, Some("system".to_string()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let probe = ConfigProbe {
            fragments: vec![line(serde_json::json!({
                "op": "add",
                "path": "/root",
                "value": "root"
            }))],
            seen: Arc::clone(&seen),
        };

        cards
            .generate(
                Box::new(move || Ok(Box::new(probe.clone()))),
                config(),
                String::new(),
                CardTrigger::Manual,
            )
            .expect("start");
        wait_until(|| {
            snapshot_events(&events)
                .iter()
                .any(|event| matches!(event, CardEvent::CardEnd { .. }))
        });

        assert_eq!(
            seen.lock().expect("seen").as_slice(),
            &[(Some(CARD_MAX_TOKENS), false, 0)]
        );
    }

    /// A provider that parks inside `chat` until released (single-flight).
    struct GatedProvider {
        started: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
    }

    impl Provider for GatedProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[slimcode_app::ToolSpec],
            _config: &ProviderConfig,
            _cancel: &CancelToken,
            _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.started.store(true, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(None)
        }
    }

    /// A provider that parks until the cancel token is set (a turn starting).
    struct CancelAwareProvider {
        started: Arc<AtomicBool>,
    }

    impl Provider for CancelAwareProvider {
        fn chat(
            &mut self,
            _messages: &[Message],
            _tools: &[slimcode_app::ToolSpec],
            _config: &ProviderConfig,
            cancel: &CancelToken,
            _on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<Option<TokenUsage>, String> {
            self.started.store(true, Ordering::SeqCst);
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(None)
        }
    }
}
