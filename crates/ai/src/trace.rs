//! The trace seam (ADR-0031 D4): a required, non-degrading collaborator the
//! engine uses to open spans around its operand calls.
//!
//! A host injects a [`Tracer`] backed by any sink it likes; the loop skeleton
//! (in `slimcode-core`) opens a RAII span at each operand site — `iteration`,
//! `provider.chat`, `tool.{name}` — and closes it when the span's
//! [`SpanRecorder`] drops. With no backend every span is a silent no-op that
//! never errors, so the code-type preset (which passes no tracer) is
//! byte-identical to a run without tracing at all.
//!
//! These types live in `slimcode-ai` (the crate that owns the [`Provider`]
//! trait, ADR-0011 D1) because a provider needs the [`SpanRecorder`] handle on
//! the seam: it records the serialized request body onto the current
//! `provider.chat` span so those attributes survive even a failed transport.
//! `slimcode-core` re-exports them so the kernel and hosts name them from the
//! runtime module.

/// Attributes collected on a span: ordered key/value string pairs.
pub type SpanAttributes = Vec<(String, String)>;

/// The callback a [`SpanRecorder`] runs when its span ends: it receives the
/// span name and every attribute recorded on it.
type OnEnd = Box<dyn FnOnce(&str, SpanAttributes) + Send + 'static>;

/// A recording handle for one in-flight span. The kernel opens it at an
/// operand-call site via [`Tracer::start`]; the span **ends when this recorder
/// drops** (RAII), handing its collected attributes to the `on_end` callback the
/// tracer supplied. Because the drop also runs on the error path, a provider
/// that records the request body before the transport call still lands those
/// attributes on the span when the request fails (ADR-0031 D4).
///
/// A recorder built by [`SpanRecorder::noop`] flushes to nothing — that is the
/// no-backend path the kernel uses when no [`Tracer`] is wired in, so an operand
/// always receives *some* recorder and never has to branch on tracing being on.
pub struct SpanRecorder {
    name: String,
    attributes: SpanAttributes,
    on_end: Option<OnEnd>,
}

impl SpanRecorder {
    /// Start a span named `name`; when the returned recorder drops, `on_end`
    /// receives the span name and every attribute recorded on it.
    pub fn start(
        name: impl Into<String>,
        on_end: impl FnOnce(&str, SpanAttributes) + Send + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            attributes: Vec::new(),
            on_end: Some(Box::new(on_end)),
        }
    }

    /// The no-op recorder: it records attributes into a void and closes
    /// silently. The kernel hands one of these to an operand when there is no
    /// backend, so the seam never needs a tracing on/off branch.
    pub fn noop() -> Self {
        Self {
            name: String::new(),
            attributes: Vec::new(),
            on_end: None,
        }
    }

    /// The span's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Attach a key/value attribute to this span.
    pub fn record(&mut self, key: &str, value: &str) {
        self.attributes.push((key.to_string(), value.to_string()));
    }
}

impl Drop for SpanRecorder {
    fn drop(&mut self) {
        if let Some(on_end) = self.on_end.take() {
            on_end(&self.name, std::mem::take(&mut self.attributes));
        }
    }
}

/// The injected tracing collaborator (ADR-0031 D4). The kernel calls
/// [`Tracer::start`] at each operand site and closes each span when the returned
/// [`SpanRecorder`] drops. A host that wants no tracing passes `None` for the
/// tracer, or a `Tracer` whose [`Tracer::start`] returns `None`; either way
/// every span is a silent no-op. Object-safe so it crosses the seam behind an
/// `Arc` (the recorder's `on_end` is a boxed callback, keeping the trait free of
/// associated types and generic methods).
pub trait Tracer: Send + Sync {
    /// Begin a span named `name`, or `None` to trace nothing.
    fn start(&self, name: &str) -> Option<SpanRecorder>;
}
