# Provider-owned concerns ride a config seam; the provider owns its wire quirks

Adding a second provider would have exposed a boundary nobody had written down: which settings belong
to the LLM endpoint and which belong to the runner? The `cache` flag had landed in `BailianConfig`
(the endpoint config), which was right in spirit but wrong in two ways — the type name leaked the
concrete provider into `app`/`cli`, and nothing stopped provider settings from accumulating in
`RunConfig` (the runner's runtime config) instead. At the same time, the request-side
`cache_control` mark — where it goes and how it is shaped — is a wire quirk of one endpoint, not a
runtime concern.

This ADR fixes the three-way split and makes the provider stateless so the settings actually cross
the seam on every call.

## Decisions

### D1 — Three kinds of setting, told apart by one question

Ask: *"if I swapped the provider, would this behave differently?"*

- **Yes → provider-owned** (`ProviderConfig`, `slimcode-ai`): api key, base URL, model, and the
  explicit-cache flag. The cache mark's syntax *and* placement also live here — the provider decides
  which messages carry `cache_control` and in what shape, because that is exactly what changes when
  the endpoint changes.
- **No, the runner runs differently → runtime config** (`RunConfig`, `slimcode-core`): currently only
  `parallel_tools`. `RunConfig` must never grow a provider setting.
- **Neither, it is application composition** (`app`/`cli`): model *name* already flows through
  `ProviderConfig`; the choice of which provider to build stays in `app::setup`.

### D2 — `BailianConfig` is renamed `ProviderConfig`; the name stops leaking

The type is pure provider data (`api_key`, `base_url`, `model`, `cache`), its 3-arg `new`, its
`with_cache(bool)` builder, its `cache: true` default and its `chat_completions_url()` all keep their
current shape — only the name changes, from `BailianConfig` to `ProviderConfig`. `app` and `cli`
must not know which provider sits below the seam; `BailianProvider` (the concrete implementation)
stays a `slimcode-ai` detail that only `app::setup` names.

### D3 — The provider is stateless; config crosses `chat`

`BailianProvider::new()` builds only the HTTP client and takes no config. `Provider::chat` gains a
`config: &ProviderConfig` parameter, and the provider reads model / base URL / api key / cache from
it on every call. One instance is therefore reusable across configs — which is what makes the seam
testable (a fake provider sees the exact resolved config) and what a future runtime config switch
needs. `AgentRunner` and `run_turn` carry the config alongside `cancel` and hand it to every `chat`.

### D4 — No tool sort: order is already deterministic

The tool list is serialized in the order it is declared (a fixed tool set; skills sorted by name;
context files root-first). Since no ordering nondeterminism exists today, this ADR does **not**
introduce a canonical sort — a model-visible reordering would be a behavior change, and it would sit
awkwardly beside the cache-off byte-identity promise. When dynamic tools (e.g. MCP) make order
genuinely unstable, canonicalization is a provider-owned concern to decide then.

### D5 — Byte determinism is a contract, not a coincidence

For the same messages + tools + cache flag, the request must serialize to byte-identical bytes,
because a cached prefix only stays valid while its bytes stay stable. The wire boundary is pinned by
tests that serialize the full request twice and by a test that locks the complete JSON literal
(both marks included). Nothing time-, order- or map-iteration-dependent may leak into the request.

### D6 — Bailian needs array-form content for every text message, not just the marked ones

Bailian only accepts `cache_control` on **array-form** content (`[{"type":"text",...}]`), and its
explicit cache matches messages by content blocks. If only the marked messages used array form, a
message promoted from "last (marked array)" to "history (unmarked string)" would change its bytes on
the next turn and break the prefix match — the history cache would never hit. So when cache is on,
**every message carrying non-empty text serializes as array form**; the presence or absence of
`cache_control` (comparison-exempt metadata) is the only difference. With cache off, every message
keeps the pre-cache plain-string shape, so the feature stays purely additive. This is a deliberate
narrowing of the "bytes differ only by the mark" wording from the originating spec.

## Considered Options

- **Keep `cache` in `RunConfig`** — rejected: it is not a runner behavior; it is an endpoint
  request-shaping flag, and `RunConfig` is shared by every frontend that might use a different
  provider.
- **Keep `BailianConfig` and re-export it from `app` as `ProviderConfig`** — rejected: the alias
  would drift and the concrete name would keep appearing in `app`/`cli` signatures.
- **Keep the provider holding its config (constructed with it)** — rejected: it makes the config
  invisible at the `chat` call site, forces construction to own settings, and blocks instance reuse
  across configs.
- **Mark only the true last message, no tail scan** — rejected: Bailian rejects a mark on empty
  content, and a trailing assistant tool-call / empty tool result would then silently lose the
  cache for the whole historical prefix.
- **Sort tools canonically now** — rejected: no nondeterminism to fix, and it changes model-visible
  bytes for no gain (D4).

## Consequences

- `ProviderConfig` is the sole provider-setting carrier; `RunConfig` is asserted (by test and by
  convention) to hold runtime behavior only.
- `Provider::chat` takes four arguments and the provider is stateless; every fake provider in the
  test suite shows the resolved config reaching the seam. *(The count is now five — ADR-0019 adds the
  delta sink; the config still crosses the seam exactly as decided here.)*
- Cache-off request bytes are byte-identical to the pre-cache client; cache-on bytes are array-form
  throughout with marks on the system message and the last cache-able conversation message.
- The mark placement strategy is a provider implementation detail: a future provider may place marks
  differently without touching `core`/`app`/`cli`.
- `cached_tokens` reporting is unchanged — the new mark is observable through the existing usage
  path.
