//! Auto-compaction: token estimation, the threshold check, and the LLM-backed
//! summarization that replaces an old span of history with one
//! [`AgentMessage::CompactSummary`].
//!
//! The feature has three parts (spec `.scratch/compact`):
//! 1. pure estimation + threshold helpers ([`estimate_message_tokens`],
//!    [`estimate_total_tokens`], [`should_compact`]) using pi's conservative
//!    `chars / 4` heuristic;
//! 2. [`compact_messages`], which cuts history at a recent-keep boundary,
//!    asks the provider for a structured checkpoint summary (pi's prompt
//!    template), and returns `[CompactSummary, ...kept]`;
//! 3. the frontend decides *when* to call it (end of a completed turn, or
//!    `/compact`) — this module owns *what* compaction does.
//!
//! A [`CompactSummary`](AgentMessage::CompactSummary) is session-only:
//! [`AgentMessage::to_llm`] drops it, and `ContextBuilder` injects its text as
//! a `user` message at the variant's position, so the model reads the summary
//! instead of the span it replaced.

use std::fmt::Write as _;

use slimcode_ai::ProviderConfig;
use slimcode_core::agent::{CancelToken, Delta, Provider};
use slimcode_core::session::{AgentMessage, Message, Role};

/// Estimated context window (tokens) driving the compaction threshold. A
/// single constant covers the mainstream models slimcode targets; the spec
/// deliberately makes it non-configurable for v1.
pub const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

/// Compact once the estimated history exceeds this percentage of the window.
const COMPACT_THRESHOLD_PERCENT: usize = 92;

/// Share of the window kept as recent history after a compaction.
const KEEP_RECENT_PERCENT: usize = 8;

/// Conservative response cap for the summarization call (spec §5): a summary
/// never needs to be long, and a bounded response keeps the compaction cheap.
const SUMMARY_MAX_TOKENS: u32 = 2048;

/// Maximum characters of a tool result serialized into the summarization
/// prompt (pi's `TOOL_RESULT_MAX_CHARS`): tool output dominates context size
/// and is not needed in full to summarize.
const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// The estimated context window, in tokens.
pub fn estimated_context_window() -> usize {
    DEFAULT_CONTEXT_WINDOW
}

/// Estimate one message's tokens with the `chars / 4` heuristic (rounded up),
/// counting the text and every tool call's name plus raw arguments. Message
/// text alone misses an assistant turn that is only a large `edit` call, which
/// is exactly the kind of message compaction must account for.
pub fn estimate_message_tokens(msg: &AgentMessage) -> usize {
    let mut chars = msg.text_content().chars().count();
    for call in msg.tool_calls() {
        chars += call.name.chars().count();
        chars += call.arguments.chars().count();
    }
    chars.div_ceil(4)
}

/// Sum [`estimate_message_tokens`] over a history.
pub fn estimate_total_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Whether this history crossed the compaction threshold. A history whose
/// newest entry is already a [`AgentMessage::CompactSummary`] never triggers
/// again: the summary plus its kept tail sits far below the threshold, and
/// the guard makes an accidental re-run on the same history a no-op.
pub fn should_compact(messages: &[AgentMessage]) -> bool {
    if messages
        .last()
        .is_some_and(AgentMessage::is_compact_summary)
    {
        return false;
    }
    estimate_total_tokens(messages) > estimated_context_window() * COMPACT_THRESHOLD_PERCENT / 100
}

/// The recent-history token budget kept after a compaction.
pub fn keep_recent_tokens() -> usize {
    estimated_context_window() * KEEP_RECENT_PERCENT / 100
}

/// The index of the first message to keep when compacting `messages`: walk
/// backwards accumulating tokens up to [`keep_recent_tokens`]. It is `0` when
/// the whole history already fits (nothing to compact) and `messages.len()`
/// when even the newest message alone exceeds the budget (summarize all of it).
/// The boundary then advances past any leading tool result, because a kept
/// result must never lose the assistant tool call it answers (that would break
/// the next request's pairing).
pub(crate) fn kept_start(messages: &[AgentMessage]) -> usize {
    let budget = keep_recent_tokens();
    let mut accumulated = 0usize;
    let mut start = messages.len();
    for i in (0..messages.len()).rev() {
        let tokens = estimate_message_tokens(&messages[i]);
        if accumulated + tokens > budget {
            break;
        }
        accumulated += tokens;
        start = i;
    }
    while start < messages.len() && messages[start].role() == &Role::Tool {
        start += 1;
    }
    start
}

/// Compact `messages`: summarize the old span with the provider and replace it
/// with one [`AgentMessage::CompactSummary`], keeping the recent tail.
///
/// `previous_summary` is read from the newest compaction checkpoint already in
/// `messages` (if any); the request then asks the LLM to *update* that summary
/// rather than regenerate it. The superseded checkpoint is not kept in the
/// tail — the new summary already carries it.
///
/// Errors (provider failure, cancellation, a refused/empty summary) leave the
/// caller's history untouched; the caller decides whether to retry.
pub fn compact_messages<P: Provider>(
    provider: &mut P,
    config: &ProviderConfig,
    cancel: &CancelToken,
    messages: &[AgentMessage],
) -> Result<Vec<AgentMessage>, String> {
    let tokens_before = estimate_total_tokens(messages);
    let start = kept_start(messages);
    if start == 0 {
        return Err("nothing to compact: the history fits the retained budget".to_string());
    }
    // The span being replaced: everything before the kept tail. Compaction
    // checkpoints are not conversation text — their summary rides the prompt
    // as `previous_summary` instead.
    let to_summarize: Vec<AgentMessage> = messages[..start]
        .iter()
        .filter(|m| !m.is_compact_summary())
        .cloned()
        .collect();
    if to_summarize.is_empty() {
        // The only eligible span was an earlier summary itself; there is no
        // new conversation text to fold in (`/compact` on a just-compacted
        // session lands here).
        return Err("nothing to compact: the history fits the retained budget".to_string());
    }
    let previous_summary = messages.iter().rev().find_map(|m| match m {
        AgentMessage::CompactSummary { summary, .. } => Some(summary.clone()),
        _ => None,
    });
    let summary = generate_summary(
        provider,
        config,
        cancel,
        &to_summarize,
        previous_summary.as_deref(),
    )?;
    // Keep the recent tail, dropping any superseded checkpoint from it.
    let mut compacted = Vec::with_capacity(messages.len() - start + 1);
    compacted.push(AgentMessage::compact_summary(
        summary,
        tokens_before,
        previous_summary,
    ));
    compacted.extend(
        messages[start..]
            .iter()
            .filter(|m| !m.is_compact_summary())
            .cloned(),
    );
    Ok(compacted)
}

/// Ask the provider for the structured summary of `to_summarize`, merging
/// `previous_summary` when present. Collects the streamed text; reasoning
/// deltas and tool calls are ignored (the summarization request declares no
/// tools).
fn generate_summary<P: Provider>(
    provider: &mut P,
    config: &ProviderConfig,
    cancel: &CancelToken,
    to_summarize: &[AgentMessage],
    previous_summary: Option<&str>,
) -> Result<String, String> {
    let conversation = serialize_conversation(to_summarize);
    let prompt = build_summarization_prompt(&conversation, previous_summary);
    let request = vec![
        Message::text(Role::System, SUMMARIZATION_SYSTEM_PROMPT),
        Message::text(Role::User, prompt),
    ];
    // One-off call (spec §5): no tools, no cache writes, bounded response.
    let config = config
        .clone()
        .with_cache(false)
        .with_max_tokens(SUMMARY_MAX_TOKENS);
    let mut text = String::new();
    provider.chat(&request, &[], &config, cancel, &mut |delta| {
        if let Delta::Text(fragment) = delta {
            text.push_str(&fragment);
        }
        Ok(())
    })?;
    if cancel.is_cancelled() {
        return Err("compaction cancelled".to_string());
    }
    let summary = text.trim().to_string();
    if summary.is_empty() {
        return Err("compaction produced an empty summary".to_string());
    }
    Ok(summary)
}

/// The fixed summarization system prompt (pi's `SUMMARIZATION_SYSTEM_PROMPT`).
const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

/// The initial summarization instruction (pi's `SUMMARIZATION_PROMPT`).
const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or \"(none)\" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// The incremental instruction (pi's `UPDATE_SUMMARIZATION_PROMPT`): merge the
/// new messages into the existing summary instead of regenerating it.
const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed
- UPDATE \"Next Steps\" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// Build the summarization user prompt: the serialized conversation in
/// `<conversation>` tags, the previous summary (when updating) in
/// `<previous-summary>` tags, then the matching instruction.
fn build_summarization_prompt(conversation: &str, previous_summary: Option<&str>) -> String {
    let mut prompt = format!("<conversation>\n{conversation}\n</conversation>\n\n");
    let instructions = match previous_summary {
        Some(previous) => {
            let _ = write!(
                prompt,
                "<previous-summary>\n{previous}\n</previous-summary>\n\n"
            );
            UPDATE_SUMMARIZATION_PROMPT
        }
        None => SUMMARIZATION_PROMPT,
    };
    prompt.push_str(instructions);
    prompt
}

/// Serialize a history as plain text so the summarization model reads it as
/// material to summarize, not as a conversation to continue (pi's
/// `serializeConversation`): one line per message, tool results truncated.
fn serialize_conversation(messages: &[AgentMessage]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for message in messages {
        // A session-only checkpoint has no wire message; only LLM turns are
        // conversation text.
        let Some(wire) = message.to_llm() else {
            continue;
        };
        match wire.role {
            Role::User => {
                let text = wire.text_content();
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            Role::Assistant => {
                let text = wire.text_content();
                if !text.is_empty() {
                    parts.push(format!("[Assistant]: {text}"));
                }
                if !wire.tool_calls.is_empty() {
                    let calls: Vec<String> = wire
                        .tool_calls
                        .iter()
                        .map(|call| {
                            format!("{}({})", call.name, format_tool_arguments(&call.arguments))
                        })
                        .collect();
                    parts.push(format!("[Assistant tool calls]: {}", calls.join("; ")));
                }
            }
            Role::Tool => {
                let text = wire.text_content();
                if !text.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&text, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            Role::System => {}
        }
    }
    parts.join("\n\n")
}

/// Render a raw JSON arguments string the way pi does: `key=value, ...` from
/// its object members, falling back to the raw string when it is not a JSON
/// object.
fn format_tool_arguments(arguments: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", "),
        _ => arguments.to_string(),
    }
}

/// Truncate long text for the summarization prompt, marking how much was cut
/// (pi's `truncateForSummary`).
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    let truncated = text.chars().count() - max_chars;
    format!("{kept}\n\n[... {truncated} more characters truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use slimcode_core::agent::{FinishReason, ToolSpec};
    use slimcode_core::session::ToolCall;

    /// A message carrying `chars` characters of text; `chars` is a multiple of
    /// four in the boundary tests so `chars / 4` is exact.
    fn text_message(chars: usize) -> AgentMessage {
        AgentMessage::text(Role::Assistant, "x".repeat(chars))
    }

    // --- estimation + threshold -------------------------------------------

    #[test]
    fn estimate_message_tokens_uses_chars_over_four_rounded_up() {
        assert_eq!(estimate_message_tokens(&text_message(0)), 0);
        assert_eq!(estimate_message_tokens(&text_message(4)), 1);
        assert_eq!(estimate_message_tokens(&text_message(5)), 2);
        assert_eq!(estimate_message_tokens(&text_message(40)), 10);
    }

    #[test]
    fn estimate_message_tokens_counts_tool_call_arguments() {
        let mut message = AgentMessage::text(Role::Assistant, "");
        if let Some(llm) = message.llm_mut() {
            llm.tool_calls.push(ToolCall {
                id: "call_1".to_string(),
                name: "edit".to_string(),
                // 8 chars name + args + text; /4.
                arguments: "{\"path\": \"a.rs\"}".to_string(),
            });
        }
        let name_chars = "edit".len();
        let arg_chars = "{\"path\": \"a.rs\"}".len();
        assert_eq!(
            estimate_message_tokens(&message),
            (name_chars + arg_chars).div_ceil(4)
        );
    }

    #[test]
    fn estimate_message_tokens_counts_a_compact_summary() {
        let message = AgentMessage::compact_summary("abcd", 0, None);
        assert_eq!(estimate_message_tokens(&message), 1);
    }

    #[test]
    fn estimate_total_tokens_sums_messages() {
        let messages = vec![text_message(4), text_message(8)];
        assert_eq!(estimate_total_tokens(&messages), 3);
        assert_eq!(estimate_total_tokens(&[]), 0);
    }

    #[test]
    fn context_window_is_the_128k_constant() {
        assert_eq!(estimated_context_window(), 128_000);
        // 92% threshold, 8% kept tail.
        assert_eq!(keep_recent_tokens(), 10_240);
    }

    #[test]
    fn should_compact_at_the_threshold_boundary() {
        let threshold = estimated_context_window() * 92 / 100; // 117_760
        // 91%: below.
        let below = vec![text_message((threshold * 91 / 100) * 4)];
        assert!(!should_compact(&below));
        // Exactly 92%: `>` is strict, so still not compacting.
        let exact = vec![text_message(threshold * 4)];
        assert_eq!(estimate_total_tokens(&exact), threshold);
        assert!(!should_compact(&exact));
        // 100% of the window: compacting.
        let full = vec![text_message(estimated_context_window() * 4)];
        assert!(should_compact(&full));
    }

    #[test]
    fn should_compact_is_false_once_the_newest_entry_is_a_summary() {
        let huge = vec![
            text_message(estimated_context_window() * 4),
            AgentMessage::compact_summary("small", 1, None),
        ];
        assert!(
            !should_compact(&huge),
            "a trailing checkpoint never re-triggers compaction"
        );
    }

    #[test]
    fn should_compact_counts_a_checkpoint_that_follows_huge_history() {
        // The load boundary drops records a checkpoint replaced, so this shape
        // only arises in a hand-built history; the threshold counts whatever it
        // is given (ticket 02).
        let history = vec![
            text_message(estimated_context_window() * 4),
            AgentMessage::compact_summary("summary", 1, None),
            AgentMessage::text(Role::User, "later"),
        ];
        assert!(should_compact(&history));
    }

    // --- compaction execution ---------------------------------------------

    /// A provider that records the request it received and returns the
    /// scripted text.
    struct ScriptedProvider {
        reply: String,
        seen: Option<Vec<Message>>,
        seen_config: Option<ProviderConfig>,
        fail: bool,
    }

    impl ScriptedProvider {
        fn replying(reply: &str) -> Self {
            Self {
                reply: reply.to_string(),
                seen: None,
                seen_config: None,
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                reply: String::new(),
                seen: None,
                seen_config: None,
                fail: true,
            }
        }
    }

    impl Provider for ScriptedProvider {
        fn chat(
            &mut self,
            messages: &[Message],
            _tools: &[ToolSpec],
            config: &ProviderConfig,
            _cancel: &CancelToken,
            on_delta: &mut dyn FnMut(Delta) -> Result<(), String>,
        ) -> Result<(), String> {
            if self.fail {
                return Err("provider exploded".to_string());
            }
            self.seen = Some(messages.to_vec());
            self.seen_config = Some(config.clone());
            on_delta(Delta::Text(self.reply.clone()))?;
            on_delta(Delta::Done(FinishReason::Stop))
        }
    }

    /// A history long enough to compact: many small turns, plus a big one so
    /// the keep boundary falls mid-list.
    fn compactable_history() -> Vec<AgentMessage> {
        let mut messages = Vec::new();
        for i in 0..40 {
            messages.push(AgentMessage::text(Role::User, format!("question {i}")));
            messages.push(AgentMessage::text(
                Role::Assistant,
                format!("answer {} {}", i, "y".repeat(2000)),
            ));
        }
        messages
    }

    fn test_config() -> ProviderConfig {
        ProviderConfig::new("test-key", "https://example.invalid/v1", "test-model")
    }

    #[test]
    fn compact_messages_replaces_the_old_span_and_keeps_the_tail() {
        let history = compactable_history();
        let tokens_before = estimate_total_tokens(&history);
        let mut provider = ScriptedProvider::replying("## Goal\nfinish\n");
        let compacted =
            compact_messages(&mut provider, &test_config(), &CancelToken::new(), &history).unwrap();

        // The result starts with the checkpoint and keeps a recent tail.
        assert!(compacted[0].is_compact_summary());
        match &compacted[0] {
            AgentMessage::CompactSummary {
                summary,
                tokens_before: recorded,
                previous_summary,
            } => {
                assert_eq!(summary, "## Goal\nfinish");
                assert_eq!(*recorded, tokens_before);
                assert_eq!(previous_summary, &None);
            }
            other => panic!("expected a checkpoint, got {other:?}"),
        }
        assert!(compacted.len() > 1, "the recent tail is kept");
        assert!(compacted.len() < history.len(), "the old span is gone");
        // The tail is a suffix of the original history.
        let kept = &compacted[1..];
        assert_eq!(kept, &history[history.len() - kept.len()..]);
        // Every kept message is a wire message (no stray checkpoints).
        assert!(kept.iter().all(|m| !m.is_compact_summary()));
    }

    #[test]
    fn compact_messages_serializes_the_conversation_and_requests_update() {
        let history = compactable_history();
        let mut provider = ScriptedProvider::replying("updated");
        compact_messages(&mut provider, &test_config(), &CancelToken::new(), &history).unwrap();
        let request = provider.seen.expect("the provider was called");
        assert_eq!(request[0].role, Role::System);
        assert!(request[0].text_content().contains("context summarization"));
        assert_eq!(request[1].role, Role::User);
        let prompt = request[1].text_content();
        assert!(prompt.contains("<conversation>"), "{prompt}");
        assert!(prompt.contains("[User]: question 0"), "{prompt}");
        assert!(prompt.contains("[Assistant]: answer 0"), "{prompt}");
        assert!(prompt.contains("## Goal"), "{prompt}");
        // No previous summary yet: the initial prompt, not the update one.
        assert!(!prompt.contains("<previous-summary>"), "{prompt}");
        assert!(!prompt.contains("incorporate into the existing summary"));
    }

    #[test]
    fn compact_messages_merges_a_previous_summary() {
        let mut history = compactable_history();
        history.insert(
            0,
            AgentMessage::compact_summary("## Goal\nolder goal", 1, None),
        );
        let mut provider = ScriptedProvider::replying("merged");
        let compacted =
            compact_messages(&mut provider, &test_config(), &CancelToken::new(), &history).unwrap();

        // The prompt carries the old summary and the update instruction.
        let prompt = provider.seen.unwrap()[1].text_content();
        assert!(prompt.contains("<previous-summary>\n## Goal\nolder goal\n</previous-summary>"));
        assert!(prompt.contains("incorporate into the existing summary"));

        // The new checkpoint records the superseded summary, and no stale
        // checkpoint survives in the kept tail.
        match &compacted[0] {
            AgentMessage::CompactSummary {
                previous_summary, ..
            } => assert_eq!(previous_summary.as_deref(), Some("## Goal\nolder goal")),
            other => panic!("expected a checkpoint, got {other:?}"),
        }
        assert!(compacted.iter().skip(1).all(|m| !m.is_compact_summary()));
    }

    #[test]
    fn compact_messages_uses_a_bounded_uncached_request() {
        let mut provider = ScriptedProvider::replying("summary");
        compact_messages(
            &mut provider,
            &test_config(),
            &CancelToken::new(),
            &compactable_history(),
        )
        .unwrap();
        let config = provider.seen_config.expect("config seen");
        assert_eq!(config.max_tokens, Some(SUMMARY_MAX_TOKENS));
        assert!(!config.cache, "a one-off summary does not warm the cache");
    }

    #[test]
    fn compact_messages_propagates_provider_failure() {
        let err = compact_messages(
            &mut ScriptedProvider::failing(),
            &test_config(),
            &CancelToken::new(),
            &compactable_history(),
        )
        .unwrap_err();
        assert!(err.contains("provider exploded"), "err: {err}");
    }

    #[test]
    fn compact_messages_rejects_an_empty_summary() {
        let err = compact_messages(
            &mut ScriptedProvider::replying("   "),
            &test_config(),
            &CancelToken::new(),
            &compactable_history(),
        )
        .unwrap_err();
        assert!(err.contains("empty summary"), "err: {err}");
    }

    #[test]
    fn compact_messages_rejects_a_history_that_fits_the_keep_budget() {
        let history = vec![
            AgentMessage::text(Role::User, "hi"),
            AgentMessage::text(Role::Assistant, "hello"),
        ];
        let mut provider = ScriptedProvider::replying("summary");
        let err = compact_messages(&mut provider, &test_config(), &CancelToken::new(), &history)
            .unwrap_err();
        assert!(err.contains("nothing to compact"), "err: {err}");
        assert!(provider.seen.is_none(), "no LLM call for an empty span");
    }

    #[test]
    fn compact_messages_rejects_an_oversized_checkpoint_with_nothing_new() {
        // The only eligible span is an earlier summary itself: there is no new
        // conversation text to fold in, so no LLM call is made.
        let history = vec![
            AgentMessage::compact_summary("x".repeat(estimated_context_window() * 4), 1, None),
            AgentMessage::text(Role::User, "small"),
        ];
        let mut provider = ScriptedProvider::replying("summary");
        let err = compact_messages(&mut provider, &test_config(), &CancelToken::new(), &history)
            .unwrap_err();
        assert!(err.contains("nothing to compact"), "err: {err}");
        assert!(provider.seen.is_none());
    }

    #[test]
    fn kept_start_never_lands_on_a_tool_result() {
        // The newest messages are a tool result whose assistant tool call is
        // too big to keep: the boundary must skip the orphaned result (so it
        // is summarized instead of kept without its call).
        let huge = 10_250usize * 4; // > the 10_240-token keep budget
        let mut assistant = Message::text(Role::Assistant, "y".repeat(huge));
        assistant.tool_calls.push(ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: "{}".to_string(),
        });
        let history = vec![
            AgentMessage::Llm(assistant),
            AgentMessage::tool_result("call_1", "body"),
        ];
        assert_eq!(
            kept_start(&history),
            history.len(),
            "keep nothing rather than orphan the result"
        );

        // A history that fits keeps everything (nothing to compact).
        let small = vec![
            AgentMessage::text(Role::User, "hi"),
            AgentMessage::text(Role::Assistant, "hello"),
        ];
        assert_eq!(kept_start(&small), 0);
    }

    #[test]
    fn compact_messages_keeps_nothing_when_the_newest_message_is_too_large() {
        // A single oversized reply cannot be partially kept: the whole span
        // (including it) is summarized, leaving only the checkpoint.
        let history = vec![
            AgentMessage::text(Role::User, "write a lot"),
            text_message(estimated_context_window() * 4),
        ];
        let mut provider = ScriptedProvider::replying("summary");
        let compacted =
            compact_messages(&mut provider, &test_config(), &CancelToken::new(), &history).unwrap();
        assert_eq!(compacted.len(), 1, "{compacted:?}");
        assert!(compacted[0].is_compact_summary());
    }

    // --- serialization ----------------------------------------------------

    #[test]
    fn serialize_conversation_renders_each_role() {
        let mut assistant = Message::text(Role::Assistant, "checking");
        assistant.tool_calls.push(ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: "{\"path\": \"a.rs\"}".to_string(),
        });
        let history = vec![
            AgentMessage::text(Role::User, "read a.rs"),
            AgentMessage::Llm(assistant),
            AgentMessage::tool_result("call_1", "fn main() {}"),
        ];
        let text = serialize_conversation(&history);
        assert_eq!(
            text,
            "[User]: read a.rs\n\n[Assistant]: checking\n\n\
             [Assistant tool calls]: read(path=\"a.rs\")\n\n[Tool result]: fn main() {}"
        );
    }

    #[test]
    fn serialize_conversation_truncates_long_tool_results() {
        let long = "z".repeat(TOOL_RESULT_MAX_CHARS + 10);
        let history = vec![AgentMessage::tool_result("call_1", long)];
        let text = serialize_conversation(&history);
        assert!(
            text.contains("[... 10 more characters truncated]"),
            "{text}"
        );
        assert!(text.starts_with("[Tool result]: zzz"));
    }

    #[test]
    fn serialize_conversation_skips_checkpoints_and_system_messages() {
        let history = vec![
            AgentMessage::llm(Message::text(Role::System, "system")),
            AgentMessage::compact_summary("old summary", 1, None),
            AgentMessage::text(Role::User, "hi"),
        ];
        assert_eq!(serialize_conversation(&history), "[User]: hi");
    }

    #[test]
    fn format_tool_arguments_renders_object_members_and_falls_back() {
        assert_eq!(
            format_tool_arguments("{\"a\": 1, \"b\": \"x\"}"),
            "a=1, b=\"x\""
        );
        assert_eq!(format_tool_arguments("not json"), "not json");
    }
}
