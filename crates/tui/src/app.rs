//! The pure TUI app core: a testable state machine that owns the transcript,
//! the input box, the status line and the view state, exposes an on-key
//! reducer and a draw-to-frame function, and implements the shared
//! [`Renderer`] trait so the shared runner can stream events straight into it.
//!
//! Nothing in this module touches a real terminal. Rendering goes through a
//! [`Frame`] supplied by the caller, so tests can drive it with ratatui's
//! `TestBackend` and assert on the frame buffer. Side effects (running a turn,
//! saving a session, ...) are surfaced as [`Effect`]s for the terminal loop to
//! fulfill.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use slimcode_agent::agent::StopReason;
use slimcode_commands::{COMMANDS, find};
use slimcode_common::render::{DisplayItem, Renderer};
use slimcode_common::skills::{Skill, SkillScope, combined_suggestions, find_skill};
use tui_textarea::{CursorMove, TextArea};
use unicode_width::UnicodeWidthChar;

/// Height in rows of the input box (including its border).
const INPUT_HEIGHT: u16 = 3;

/// Number of lines a PageUp / PageDown key scrolls the transcript by.
const PAGE_LINES: usize = 10;

/// Rows available to the status line at the bottom of the layout.
const STATUS_HEIGHT: u16 = 1;

/// Immutable status-line state shown at the bottom of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLine {
    /// Working directory the turn runs in.
    pub cwd: String,
    /// Current session id (or "none" before one is created).
    pub session_id: String,
    /// Model the provider is configured with.
    pub model: String,
    /// Whether a turn is currently running.
    pub running: bool,
}

/// One row-group in the transcript.
///
/// [`Entry::Agent`] carries a streamed [`DisplayItem`] from the shared runner;
/// [`Entry::Notice`] and [`Entry::Error`] are frontend-owned output appended by
/// the terminal loop (command results, inline failures, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A display item streamed from the shared runner.
    Agent(DisplayItem),
    /// Frontend-owned output (command results, notices).
    Notice(String),
    /// An inline error (failed turn, failed command).
    Error(String),
}

/// A side effect the terminal loop fulfills after the app processed a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Quit the TUI (Ctrl+C / Ctrl+D / `/exit`).
    Quit,
    /// Submit the given prompt to the model.
    SubmitPrompt(String),
    /// Trigger a skill turn with an optional argument.
    TriggerSkill { name: String, arg: Option<String> },
    /// Replay the last N prompts from history.
    ReplayHistory(usize),
    /// Re-run a prompt recalled from history as a fresh turn, without
    /// recording it again in input history.
    ReplayPrompt(String),
    /// Start a new session (clears the transcript).
    NewSession,
    /// Load the session with the given id (clears the transcript).
    LoadSession(String),
    /// Persist the current session.
    SaveSession,
    /// List persisted sessions.
    ListSessions,
    /// Show the usage for the last turn.
    ShowUsage,
    /// List recent prompts from history.
    ListHistory,
    /// Install a skill from the given reference.
    InstallSkill(String),
}

/// Active ↑/↓ history-recall state: the index into [`App::history`] whose
/// prompt is currently shown in the input box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recall {
    /// Index into the app's `history` snapshot. The newest entry is the last
    /// one (matching `HistoryStore` order and `/!1` = newest), so recall
    /// starts there and ↑ moves toward the front (older).
    pub index: usize,
}

/// The pure TUI app core.
pub struct App {
    /// The transcript rendered in the top pane.
    pub transcript: Vec<Entry>,
    /// The multi-line input box.
    pub input: TextArea<'static>,
    /// Status-line state.
    pub status: StatusLine,
    /// Snapshot of installed skills used for `/skill` dispatch and suggestions.
    pub skills: Vec<Skill>,
    /// Recent prompts for ↑/↓ recall, in `HistoryStore` order (oldest first,
    /// newest last — recall starts at the newest entry, matching `/!1`). The
    /// terminal loop seeds this from `HistoryStore` at startup; fresh prompts
    /// are appended here on submit, mirroring `HistoryStore::append`.
    pub history: Vec<String>,
    /// Active history-recall state, if any.
    pub recall: Option<Recall>,
    /// Lines scrolled up from the bottom of the transcript (0 = at bottom).
    pub scroll: usize,
    /// Whether the view auto-follows new output.
    pub follow: bool,
    /// The transcript content width (pane width minus its borders) from the
    /// most recent draw; 0 before the first draw. Used to wrap long lines so
    /// scroll/window row math matches what is rendered.
    content_width: u16,
}

impl App {
    /// Create a fresh app in "ready" state.
    pub fn new(
        cwd: impl Into<String>,
        session_id: impl Into<String>,
        model: impl Into<String>,
        skills: Vec<Skill>,
    ) -> Self {
        let mut app = App {
            transcript: Vec::new(),
            input: fresh_input(),
            status: StatusLine {
                cwd: cwd.into(),
                session_id: session_id.into(),
                model: model.into(),
                running: false,
            },
            skills,
            history: Vec::new(),
            recall: None,
            scroll: 0,
            follow: true,
            content_width: 0,
        };
        app.push_notice("slimcode — type /help for commands, or just start typing");
        app
    }

    /// The current input text (joined across lines).
    pub fn input_text(&self) -> String {
        self.input.lines().join("\n")
    }

    /// Mark the app as running (or not) on the status line.
    pub fn set_running(&mut self, running: bool) {
        self.status.running = running;
    }

    /// Append frontend-owned output to the transcript.
    pub fn push_notice(&mut self, text: impl Into<String>) {
        self.transcript.push(Entry::Notice(text.into()));
        self.reset_view();
    }

    /// Append an inline error entry to the transcript.
    pub fn push_error(&mut self, text: impl Into<String>) {
        self.transcript.push(Entry::Error(text.into()));
        self.reset_view();
    }

    /// Clear the transcript for a new session and point the status line at the
    /// freshly created session. The terminal loop calls this after it fulfils
    /// an [`Effect::NewSession`].
    pub fn clear_for_new_session(&mut self, id: &str) {
        self.transcript.clear();
        self.status.session_id = id.to_string();
        self.reset_view();
        self.push_notice(format!("new session: {id}"));
    }

    /// Replace the transcript with a freshly loaded session. The terminal loop
    /// calls this after it fulfils an [`Effect::LoadSession`] successfully.
    pub fn apply_loaded_session(&mut self, id: &str) {
        self.transcript.clear();
        self.status.session_id = id.to_string();
        self.reset_view();
        self.push_notice(format!("loaded session: {id}"));
    }

    /// On-key reducer: returns the effect (if any) the loop must fulfil.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Effect> {
        // Global control keys first: Ctrl+C / Ctrl+D quit from any state.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('d') => return Some(Effect::Quit),
                _ => {}
            }
        }
        match key.code {
            // ↑/↓ at the empty input enter history recall; in recall they
            // navigate; otherwise they move the text-area cursor (the recall
            // state owns them so tui-textarea's own ↑/↓ do not fight it).
            KeyCode::Up | KeyCode::Down => {
                self.handle_vertical(key);
                None
            }
            // Enter submits the buffer (or re-runs a recalled prompt as a
            // fresh turn without re-recording it); Shift+Enter inserts a
            // newline so the input box stays multi-line.
            KeyCode::Enter => self.handle_enter(key),
            KeyCode::PageUp => {
                self.scroll_up(PAGE_LINES);
                None
            }
            KeyCode::PageDown => {
                self.scroll_down(PAGE_LINES);
                None
            }
            // Everything else exits recall (if active) and goes to the text
            // area (typing, arrows, ...).
            _ => {
                self.recall = None;
                self.input.input(key);
                None
            }
        }
    }

    /// Handle ↑/↓: history recall or text-area cursor movement.
    fn handle_vertical(&mut self, key: KeyEvent) {
        if self.recall.is_some() {
            self.recall_arrow(key.code);
            return;
        }
        // Both ↑ and ↓ at the empty input enter recall at the newest entry.
        if self.input_text().is_empty() && !self.history.is_empty() {
            self.enter_recall_newest();
            return;
        }
        // Non-empty input: let tui-textarea move the cursor as usual.
        self.input.input(key);
    }

    /// Handle Enter: replay a recalled prompt, insert a newline, or submit.
    fn handle_enter(&mut self, key: KeyEvent) -> Option<Effect> {
        // Enter on a recalled prompt re-runs it as a fresh turn without
        // re-recording it (same semantics as the `/!!` / `/!N` replay path).
        if let Some(recall) = self.recall.take() {
            let text = self.history.get(recall.index).cloned()?;
            self.clear_input();
            self.push_notice(format!("> {text}"));
            return Some(Effect::ReplayPrompt(text));
        }
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            self.input.insert_newline();
            return None;
        }
        self.submit_current_input()
    }

    /// Navigate within recall: ↑ goes older, ↓ goes newer; ↓ at the newest
    /// entry exits recall back to empty editing.
    fn recall_arrow(&mut self, code: KeyCode) {
        let Some(recall) = self.recall else { return };
        let max = self.history.len().saturating_sub(1);
        match code {
            KeyCode::Up => self.load_recall(recall.index.saturating_sub(1)),
            KeyCode::Down => {
                if recall.index == max {
                    self.recall = None;
                    self.clear_input();
                } else {
                    self.load_recall(recall.index + 1);
                }
            }
            _ => {}
        }
    }

    /// Enter recall at the newest stored prompt.
    fn enter_recall_newest(&mut self) {
        let max = self.history.len().saturating_sub(1);
        self.load_recall(max);
    }

    /// Load the prompt at `index` into the input box and mark recall active.
    fn load_recall(&mut self, index: usize) {
        if index < self.history.len() {
            let text = self.history[index].clone();
            self.recall = Some(Recall { index });
            self.set_input_text(&text);
        }
    }

    /// Seed the ↑/↓ recall snapshot from `HistoryStore` entries (oldest
    /// first, newest last — recall starts at the newest entry, matching
    /// `/!1`). The terminal loop calls this at startup.
    pub fn set_history(&mut self, entries: Vec<String>) {
        self.history = entries;
    }

    /// Replace the input box contents (used when recalling a stored prompt).
    fn set_input_text(&mut self, text: &str) {
        self.input = input_with_text(text);
    }

    /// Record a freshly submitted prompt at the end of the recall snapshot
    /// (`HistoryStore` order: oldest first, newest last). Replays never
    /// record, mirroring `HistoryStore::append` on the persistence side.
    fn record_prompt(&mut self, prompt: String) {
        self.history.push(prompt);
    }

    /// Draw the whole screen into `area` of the given frame.
    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let [transcript_area, input_area, status_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(INPUT_HEIGHT),
            Constraint::Length(STATUS_HEIGHT),
        ])
        .areas(area);

        // Transcript pane (windowed to the pane height/width minus its border;
        // long lines are wrapped to the content width so nothing is truncated).
        let content_height = transcript_area.height.saturating_sub(2);
        let content_width = transcript_area.width.saturating_sub(2);
        self.content_width = content_width;
        let lines = self.visible_lines(content_height, content_width);
        let transcript = Paragraph::new(lines).block(Block::bordered().title(" transcript "));
        frame.render_widget(transcript, transcript_area);

        // Input box.
        frame.render_widget(&self.input, input_area);

        // Status line.
        let status = Paragraph::new(self.status_line_text());
        frame.render_widget(status, status_area);
    }

    /// Push a streamed display item into the transcript and re-follow.
    ///
    /// Consecutive streamed `DisplayItem::Text` (and `DisplayItem::Reasoning`)
    /// fragments merge into a single entry, so a multi-delta text stream
    /// renders as one flowing block instead of one line per delta.
    fn push_display_item(&mut self, item: DisplayItem) {
        let merged = match &item {
            DisplayItem::Text(fragment) => {
                if let Some(Entry::Agent(DisplayItem::Text(prev))) = self.transcript.last_mut() {
                    prev.push_str(fragment);
                    true
                } else {
                    false
                }
            }
            DisplayItem::Reasoning(fragment) => {
                if let Some(Entry::Agent(DisplayItem::Reasoning(prev))) = self.transcript.last_mut()
                {
                    prev.push_str(fragment);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !merged {
            self.transcript.push(Entry::Agent(item));
        }
        self.reset_view();
    }

    /// Re-anchor the view at the bottom (follow mode): used after new content
    /// is appended and after a transcript replacement.
    fn reset_view(&mut self) {
        self.scroll = 0;
        self.follow = true;
    }

    /// Scroll the transcript up `lines` and stop following.
    fn scroll_up(&mut self, lines: usize) {
        self.follow = false;
        let total = self.total_lines();
        let max_scroll = total.saturating_sub(1);
        self.scroll = (self.scroll + lines).min(max_scroll);
    }

    /// Scroll the transcript down `lines`; reaching the bottom re-follows.
    fn scroll_down(&mut self, lines: usize) {
        self.follow = false;
        self.scroll = self.scroll.saturating_sub(lines);
        if self.scroll == 0 {
            self.follow = true;
        }
    }

    /// Number of transcript rows across all entries, counting wrapped rows at
    /// the current content width (falling back to logical lines before the
    /// first draw).
    fn total_lines(&self) -> usize {
        let width = self.content_width as usize;
        if width == 0 {
            return self.transcript.iter().map(|e| entry_lines(e).len()).sum();
        }
        self.transcript
            .iter()
            .map(|e| {
                entry_lines(e)
                    .iter()
                    .map(|l| wrap_to_width(l, width).len())
                    .sum::<usize>()
            })
            .sum()
    }

    /// The window of transcript rows visible in a pane of `height` rows and
    /// `width` columns. Each entry line is wrapped to the content width first,
    /// so every returned row fits the pane and no content is truncated.
    fn visible_lines(&self, height: u16, width: u16) -> Vec<Line<'static>> {
        let height = height as usize;
        let width = width as usize;
        let total = self.total_lines();
        if total == 0 || height == 0 {
            return Vec::new();
        }
        let max_scroll = total.saturating_sub(1);
        let scroll = self.scroll.min(max_scroll);
        let end = total.saturating_sub(scroll);
        let start = end.saturating_sub(height);

        let mut all: Vec<Line> = Vec::with_capacity(total);
        for entry in &self.transcript {
            let style = entry_style(entry);
            if width == 0 {
                for line in entry_lines(entry) {
                    all.push(Line::styled(line, style));
                }
            } else {
                for line in entry_lines(entry) {
                    for row in wrap_to_width(&line, width) {
                        all.push(Line::styled(row, style));
                    }
                }
            }
        }
        all[start..end].to_vec()
    }

    /// The styled status line.
    fn status_line_text(&self) -> Line<'static> {
        let s = &self.status;
        let indicator = if s.running { "RUNNING" } else { "ready" };
        Line::from(format!(
            "{} | session {} | {} | {}",
            s.cwd, s.session_id, s.model, indicator
        ))
    }

    /// Take the current input as a prompt: echo it, clear the box, and resolve
    /// it (either a normal prompt or a `/command`). Returns the effect to run.
    fn submit_current_input(&mut self) -> Option<Effect> {
        let text = self.input_text();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        self.clear_input();
        if trimmed.starts_with('/') {
            self.handle_command(trimmed)
        } else {
            self.push_notice(format!("> {trimmed}"));
            self.record_prompt(trimmed.to_string());
            Some(Effect::SubmitPrompt(trimmed.to_string()))
        }
    }

    /// Reset the input box to a fresh empty multi-line box.
    fn clear_input(&mut self) {
        self.input = fresh_input();
    }

    /// Resolve a `/command` line (leading slash kept in `name`).
    fn handle_command(&mut self, line: &str) -> Option<Effect> {
        let (name, arg) = split_name_arg(line);
        if let Some(command) = find(name) {
            return match command.name {
                "/help" => {
                    self.show_help();
                    None
                }
                "/new" => Some(Effect::NewSession),
                "/save" => Some(Effect::SaveSession),
                "/load" => match arg {
                    Some(id) if !id.is_empty() => Some(Effect::LoadSession(id.to_string())),
                    _ => {
                        self.push_error("/load needs a session id — /sessions lists them");
                        None
                    }
                },
                "/sessions" => Some(Effect::ListSessions),
                "/usage" => Some(Effect::ShowUsage),
                "/history" => Some(Effect::ListHistory),
                "/skills" => {
                    self.show_skills();
                    None
                }
                "/install-skill" => match arg {
                    Some(reference) if !reference.is_empty() => {
                        Some(Effect::InstallSkill(reference.to_string()))
                    }
                    _ => {
                        self.push_error("/install-skill needs a skill reference");
                        None
                    }
                },
                "/exit" => Some(Effect::Quit),
                "/!!" => Some(Effect::ReplayHistory(1)),
                "/!" => match arg {
                    Some(digits) => match digits.parse::<usize>() {
                        Ok(n) if n >= 1 => Some(Effect::ReplayHistory(n)),
                        _ => {
                            self.push_error(format!("bad replay index: /!{digits}"));
                            None
                        }
                    },
                    None => {
                        self.push_error("/!N needs a number (1 = newest)");
                        None
                    }
                },
                // Every other registered command falls through to generic
                // handling: skill dispatch and predictive suggestions.
                _ => self.handle_unresolved_command(name, arg),
            };
        }
        self.handle_unresolved_command(name, arg)
    }

    /// Resolve a command name that did not match a registered command exactly:
    /// numbered replay (`/!N`), skill triggers, or an unknown-command notice.
    fn handle_unresolved_command(&mut self, name: &str, arg: Option<&str>) -> Option<Effect> {
        // Numbered replay: `/!N` (N digits) is not an exact spelling, so a
        // numbered command never matches `find` and lands here.
        if let Some(spec) = name.strip_prefix("/!")
            && !spec.is_empty()
            && spec.chars().all(|c| c.is_ascii_digit())
        {
            let n = spec.parse::<usize>().unwrap_or(1);
            return Some(Effect::ReplayHistory(n));
        }
        // Skill trigger: `/skill-name`.
        if let Some(skill) = find_skill(&self.skills, name) {
            return Some(Effect::TriggerSkill {
                name: skill.name.clone(),
                arg: arg.map(str::to_string),
            });
        }
        // Unknown command: predictive notice with suggestions.
        let suggestions = combined_suggestions(&self.skills, name);
        self.push_error(format!("unknown command: {name}"));
        if suggestions.is_empty() {
            self.push_notice("  run /help to list commands");
        } else {
            self.push_notice(format!("  did you mean: {}", suggestions.join(", ")));
        }
        None
    }

    /// Render the built-in command list.
    fn show_help(&mut self) {
        self.push_notice("commands:");
        let width = COMMANDS
            .iter()
            .map(|c| c.usage.chars().count())
            .max()
            .unwrap_or(0);
        for command in COMMANDS {
            let mut line = format!("  {:<width$}  {}", command.usage, command.description);
            if !command.aliases.is_empty() {
                line.push_str(&format!("  (alias: {})", command.aliases.join(", ")));
            }
            self.push_notice(line);
        }
        self.push_notice("multi-line: Shift+Enter inserts a newline; Enter submits");
        self.push_notice("skills: /skills lists installed skills; /<skill> runs one");
    }

    /// Render the installed skills list.
    fn show_skills(&mut self) {
        if self.skills.is_empty() {
            self.push_notice("no skills installed");
            return;
        }
        self.push_notice("skills:");
        let width = self
            .skills
            .iter()
            .map(|s| s.name.chars().count())
            .max()
            .unwrap_or(0);
        let lines: Vec<String> = self
            .skills
            .iter()
            .map(|skill| {
                let scope = match skill.scope {
                    SkillScope::User => "user",
                    SkillScope::Project => "project",
                };
                let manual = if skill.disable_model_invocation {
                    " (manual only)"
                } else {
                    ""
                };
                format!(
                    "  /{:<width$}  {}{}  [{}]",
                    skill.name, skill.description, manual, scope
                )
            })
            .collect();
        for line in lines {
            self.push_notice(line);
        }
    }
}

impl Renderer for App {
    fn render(&mut self, item: &DisplayItem) -> Result<(), String> {
        self.push_display_item(item.clone());
        Ok(())
    }
}

/// Build a fresh multi-line input box with placeholder and border.
fn fresh_input() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    decorate_input(&mut textarea);
    textarea
}

/// Build a multi-line input box pre-filled with `text` (recalled prompts).
fn input_with_text(text: &str) -> TextArea<'static> {
    let mut textarea = TextArea::new(text.lines().map(str::to_string).collect());
    decorate_input(&mut textarea);
    // Place the cursor at the end so typing continues after the prompt.
    textarea.move_cursor(CursorMove::Bottom);
    textarea.move_cursor(CursorMove::End);
    textarea
}

/// Shared input-box decoration: placeholder and bordered title.
fn decorate_input(textarea: &mut TextArea<'static>) {
    textarea.set_placeholder_text("prompt… Enter submits, Shift+Enter newline, ↑ history");
    textarea.set_block(Block::bordered().title(" input "));
}

/// Split a `/command` line into its name (leading slash kept) and optional
/// argument.
fn split_name_arg(line: &str) -> (&str, Option<&str>) {
    let mut parts = line.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let arg = parts.next().filter(|a| !a.trim().is_empty());
    (name, arg)
}

/// The plain text lines an entry contributes to the transcript.
fn entry_lines(entry: &Entry) -> Vec<String> {
    match entry {
        Entry::Agent(item) => item_lines(item),
        Entry::Notice(text) => text.lines().map(str::to_string).collect(),
        Entry::Error(text) => text.lines().map(str::to_string).collect(),
    }
}

/// Wrap `line` so it fits `width` display columns, splitting at character
/// boundaries when it would overflow. CJK and other wide characters count as
/// two columns (via `unicode-width`), matching the terminal. Returns at least
/// one row (empty input yields a single empty row) so layout stays stable.
fn wrap_to_width(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for c in line.chars() {
        let cw = c.width().unwrap_or(0);
        if current_width > 0 && current_width + cw > width {
            rows.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(c);
        current_width += cw;
    }
    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
    }
    rows
}

/// The plain text lines a display item contributes to the transcript.
fn item_lines(item: &DisplayItem) -> Vec<String> {
    match item {
        DisplayItem::Turn { turn } => vec![format!("── turn {turn} ──")],
        DisplayItem::Reasoning(text) => text
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>(),
        DisplayItem::Text(text) => text.lines().map(str::to_string).collect(),
        DisplayItem::ToolStart { name, arguments } => {
            vec![format!("  ▶ {name} {arguments}")]
        }
        DisplayItem::ToolResult { name, ok, result } => {
            let icon = if *ok { "✔" } else { "✖" };
            vec![format!("  {icon} {name}: {result}")]
        }
        DisplayItem::Stop(StopReason::Completed) => vec!["✓ done".to_string()],
        DisplayItem::Stop(StopReason::MaxIterations) => {
            vec!["⚠ stopped: max iterations reached".to_string()]
        }
        DisplayItem::Usage(usage) => vec![format!(
            "tokens: {} prompt + {} completion = {} total",
            usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
        )],
    }
}

/// The style a whole entry is rendered with.
fn entry_style(entry: &Entry) -> Style {
    match entry {
        Entry::Agent(item) => item_style(item),
        Entry::Notice(_) => Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
        Entry::Error(_) => Style::default().fg(Color::Red),
    }
}

/// The style a display item is rendered with.
fn item_style(item: &DisplayItem) -> Style {
    match item {
        DisplayItem::Turn { .. } => Style::default().fg(Color::Gray),
        DisplayItem::Reasoning(_) => Style::default().fg(Color::Yellow),
        DisplayItem::Text(_) => Style::default(),
        DisplayItem::ToolStart { .. } => Style::default().fg(Color::Cyan),
        DisplayItem::ToolResult { ok: true, .. } => Style::default().fg(Color::Green),
        DisplayItem::ToolResult { ok: false, .. } => Style::default().fg(Color::Red),
        DisplayItem::Stop(_) => Style::default().fg(Color::Green),
        DisplayItem::Usage(_) => Style::default().fg(Color::Magenta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use slimcode_ai::wire::TokenUsage;
    use slimcode_common::history::{HISTORY_DISPLAY, render_history};
    use slimcode_common::skills::{Skill, SkillScope};

    // --- helpers -----------------------------------------------------------

    /// A minimal installed skill for trigger tests.
    fn skill(name: &str, description: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: description.to_string(),
            disable_model_invocation: false,
            body: String::new(),
            scope: SkillScope::User,
            dir: std::path::PathBuf::new(),
        }
    }

    /// Render the app at `w`x`h` and return the resulting frame buffer.
    fn render_buffer(app: &mut App, w: u16, h: u16) -> Buffer {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        let completed = terminal
            .draw(|frame| app.draw(frame, frame.area()))
            .unwrap();
        completed.buffer.clone()
    }

    /// The rendered text of one buffer row.
    fn line_at(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer.cell((x, y)).unwrap().symbol().to_string())
            .collect()
    }

    /// Whether any buffer row contains `needle`.
    fn buffer_contains(buffer: &Buffer, needle: &str) -> bool {
        (0..buffer.area.height).any(|y| line_at(buffer, y).contains(needle))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Type `text` into the input box one key at a time.
    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
    }

    /// A test app with a couple of entries already in the transcript.
    fn seeded_app() -> App {
        App::new("~/proj", "sess-1", "model-x", vec![])
    }

    /// Render the transcript pane rows (content only, border excluded) of a
    /// seeded app at a fixed size, returning the visible content rows.
    fn transcript_window(app: &mut App, h: u16) -> Vec<String> {
        let buffer = render_buffer(app, 60, h);
        // The transcript pane is the top region: total height minus input box
        // and status line. Rows 0 and (pane_h - 1) are the pane's borders.
        let pane_h = h - INPUT_HEIGHT - STATUS_HEIGHT;
        let mut rows = Vec::new();
        for y in 1..(pane_h - 1) {
            rows.push(line_at(&buffer, y));
        }
        rows
    }

    // --- tests -------------------------------------------------------------

    #[test]
    fn transcript_accumulates_streamed_text() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Text("hello ".to_string()))
            .unwrap();
        app.render(&DisplayItem::Text("world\n".to_string()))
            .unwrap();

        let buffer = render_buffer(&mut app, 40, 12);
        // Consecutive streamed text fragments merge onto one transcript line.
        assert!(buffer_contains(&buffer, "hello world"));
        // A single streamed item spanning multiple lines renders fully.
        let mut app = seeded_app();
        app.render(&DisplayItem::Text("one\ntwo\n".to_string()))
            .unwrap();
        let buffer = render_buffer(&mut app, 40, 12);
        assert!(buffer_contains(&buffer, "one"));
        assert!(buffer_contains(&buffer, "two"));
    }

    #[test]
    fn transcript_accumulates_streamed_reasoning() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Reasoning("The ".to_string()))
            .unwrap();
        app.render(&DisplayItem::Reasoning("user said".to_string()))
            .unwrap();

        let buffer = render_buffer(&mut app, 60, 12);
        // Consecutive reasoning deltas merge onto one prefixed line.
        assert!(buffer_contains(&buffer, "> The user said"));
    }

    #[test]
    fn long_line_wraps_instead_of_truncating() {
        let mut app = seeded_app();
        // A line far wider than the 40-wide pane (content width 38).
        let long = format!("{}END", "x".repeat(60));
        app.render(&DisplayItem::Text(long)).unwrap();
        let buffer = render_buffer(&mut app, 40, 12);
        // The tail must be visible: wrapped, not truncated.
        assert!(buffer_contains(&buffer, "END"));
    }

    #[test]
    fn wide_chars_wrap_by_display_width() {
        let mut app = seeded_app();
        // 31 CJK chars = 62 display columns, wider than the 38-column content
        // area: the trailing char must survive via width-aware wrapping.
        let long = format!("{}尾", "好".repeat(30));
        app.render(&DisplayItem::Text(long)).unwrap();
        let buffer = render_buffer(&mut app, 40, 12);
        assert!(buffer_contains(&buffer, "尾"));
    }

    #[test]
    fn tool_and_stop_markers_render_distinct() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "read".to_string(),
            arguments: "a.txt".to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "read".to_string(),
            ok: true,
            result: "ok".to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "write".to_string(),
            ok: false,
            result: "denied".to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::Stop(StopReason::Completed))
            .unwrap();

        let buffer = render_buffer(&mut app, 60, 14);
        assert!(buffer_contains(&buffer, "▶ read a.txt"));
        assert!(buffer_contains(&buffer, "✔ read: ok"));
        assert!(buffer_contains(&buffer, "✖ write: denied"));
        assert!(buffer_contains(&buffer, "✓ done"));
    }

    #[test]
    fn enter_submits_current_input_and_clears_box() {
        let mut app = seeded_app();
        type_text(&mut app, "explain tests");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            effect,
            Some(Effect::SubmitPrompt("explain tests".to_string()))
        );
        assert!(app.input_text().is_empty());
        // The prompt is echoed into the transcript.
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(buffer_contains(&buffer, "> explain tests"));
    }

    #[test]
    fn shift_enter_inserts_multiline_input() {
        let mut app = seeded_app();
        type_text(&mut app, "first");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        type_text(&mut app, "second");
        assert_eq!(app.input_text(), "first\nsecond");

        // Enter now submits the whole multi-line buffer as a single prompt.
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            effect,
            Some(Effect::SubmitPrompt("first\nsecond".to_string()))
        );
        assert!(app.input_text().is_empty());
    }

    #[test]
    fn ctrl_c_and_ctrl_d_quit() {
        let mut app = seeded_app();
        assert_eq!(app.handle_key(ctrl_key('c')), Some(Effect::Quit));
        assert_eq!(app.handle_key(ctrl_key('d')), Some(Effect::Quit));
    }

    #[test]
    fn empty_enter_does_nothing() {
        let mut app = seeded_app();
        assert_eq!(app.handle_key(key(KeyCode::Enter)), None);
        assert!(app.input_text().is_empty());
    }

    #[test]
    fn auto_scroll_follows_and_yields_to_page_keys() {
        let mut app = seeded_app();
        // Seed 12 single-line items so the transcript is taller than the pane.
        for i in 0..12 {
            app.render(&DisplayItem::Text(format!("line {i}\n")))
                .unwrap();
        }

        // Pane shows only the last couple of rows: follow is on.
        let window = transcript_window(&mut app, 8);
        let last = window.last().unwrap().to_string();
        assert!(last.contains("line 11"), "expected newest, got {last:?}");

        // PageUp stops following and scrolls up (window shifts to older lines).
        app.handle_key(key(KeyCode::PageUp));
        let window = transcript_window(&mut app, 8);
        let last = window.last().unwrap().to_string();
        assert!(
            !last.contains("line 11"),
            "expected scrolled up, got {last:?}"
        );

        // PageDown returns to the bottom and re-follows.
        app.handle_key(key(KeyCode::PageDown));
        let window = transcript_window(&mut app, 8);
        let last = window.last().unwrap().to_string();
        assert!(last.contains("line 11"), "expected bottom, got {last:?}");

        // A new streamed item re-follows to the newest line.
        app.render(&DisplayItem::Text("line 12\n".to_string()))
            .unwrap();
        let window = transcript_window(&mut app, 8);
        let last = window.last().unwrap().to_string();
        assert!(last.contains("line 12"), "expected newest, got {last:?}");
    }

    #[test]
    fn resize_re_renders_layout() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Text("resize me\n".to_string()))
            .unwrap();

        let small = render_buffer(&mut app, 60, 10);
        assert!(buffer_contains(&small, "resize me"));
        assert!(buffer_contains(&small, "ready"));

        let wide = render_buffer(&mut app, 100, 16);
        assert!(buffer_contains(&wide, "resize me"));
        assert!(buffer_contains(&wide, "ready"));
    }

    #[test]
    fn slash_new_clears_transcript() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Text("old content\n".to_string()))
            .unwrap();

        type_text(&mut app, "/new");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, Some(Effect::NewSession));

        // The loop fulfils the effect by clearing the transcript.
        app.clear_for_new_session("sess-2");
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(!buffer_contains(&buffer, "old content"));
        assert!(buffer_contains(&buffer, "new session: sess-2"));
        assert_eq!(app.status.session_id, "sess-2");
    }

    #[test]
    fn slash_load_clears_transcript() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Text("old content\n".to_string()))
            .unwrap();

        type_text(&mut app, "/load sess-9");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, Some(Effect::LoadSession("sess-9".to_string())));

        app.apply_loaded_session("sess-9");
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(!buffer_contains(&buffer, "old content"));
        assert!(buffer_contains(&buffer, "loaded session: sess-9"));
        assert_eq!(app.status.session_id, "sess-9");
    }

    #[test]
    fn slash_load_without_id_is_an_error() {
        let mut app = seeded_app();
        type_text(&mut app, "/load");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, None);
        let buffer = render_buffer(&mut app, 80, 12);
        assert!(buffer_contains(&buffer, "/load needs a session id"));
    }

    #[test]
    fn failed_turn_appends_error_and_returns_to_input() {
        let mut app = seeded_app();
        // The user submits a prompt; the loop starts the turn and it fails.
        type_text(&mut app, "do the thing");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            effect,
            Some(Effect::SubmitPrompt("do the thing".to_string()))
        );

        app.set_running(true);
        app.push_error("provider exploded: model unreachable");
        app.set_running(false);

        let buffer = render_buffer(&mut app, 80, 12);
        assert!(buffer_contains(
            &buffer,
            "provider exploded: model unreachable"
        ));
        // The input box is empty again and ready for the next prompt.
        assert!(app.input_text().is_empty());
        assert!(!app.status.running);
    }

    #[test]
    fn slash_help_and_slash_skills_render_lists() {
        let mut app = seeded_app();
        type_text(&mut app, "/help");
        assert_eq!(app.handle_key(key(KeyCode::Enter)), None);
        let buffer = render_buffer(&mut app, 100, 30);
        // The help list is long, so assert on content anchored near the end of
        // the list (the header may have scrolled off the pane).
        assert!(buffer_contains(&buffer, "/!!"));
        assert!(buffer_contains(&buffer, "Shift+Enter"));
        assert!(buffer_contains(&buffer, "/<skill> runs one"));

        let mut app = App::new(
            "~/proj",
            "sess-1",
            "model-x",
            vec![skill("grill", "stress-test a plan")],
        );
        type_text(&mut app, "/skills");
        assert_eq!(app.handle_key(key(KeyCode::Enter)), None);
        let buffer = render_buffer(&mut app, 100, 20);
        assert!(buffer_contains(&buffer, "skills:"));
        assert!(buffer_contains(&buffer, "/grill"));
    }

    #[test]
    fn unknown_command_suggests_and_error_renders() {
        let mut app = seeded_app();
        type_text(&mut app, "/nop");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, None);
        let buffer = render_buffer(&mut app, 100, 12);
        assert!(buffer_contains(&buffer, "unknown command: /nop"));
        assert!(buffer_contains(&buffer, "/help"));
    }

    #[test]
    fn slash_exit_quits() {
        let mut app = seeded_app();
        type_text(&mut app, "/exit");
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(Effect::Quit));
    }

    #[test]
    fn slash_save_sessions_usage_history_effects() {
        let mut app = seeded_app();
        type_text(&mut app, "/save");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::SaveSession)
        );
        type_text(&mut app, "/sessions");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::ListSessions)
        );
        type_text(&mut app, "/usage");
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(Effect::ShowUsage));
        type_text(&mut app, "/history");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::ListHistory)
        );
        type_text(&mut app, "/install-skill github:org/repo");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::InstallSkill("github:org/repo".to_string()))
        );
    }

    #[test]
    fn skill_trigger_returns_effect() {
        let mut app = App::new(
            "~/proj",
            "sess-1",
            "model-x",
            vec![skill("grill", "stress-test a plan")],
        );
        type_text(&mut app, "/grill my plan");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            effect,
            Some(Effect::TriggerSkill {
                name: "grill".to_string(),
                arg: Some("my plan".to_string()),
            })
        );
    }

    #[test]
    fn numbered_and_rerun_replay_effects() {
        let mut app = seeded_app();
        type_text(&mut app, "/!!");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::ReplayHistory(1))
        );
        type_text(&mut app, "/!3");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::ReplayHistory(3))
        );
        type_text(&mut app, "/!x");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, None);
        let buffer = render_buffer(&mut app, 80, 12);
        assert!(buffer_contains(&buffer, "unknown command: /!x"));
    }

    #[test]
    fn usage_renders_token_counts() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Usage(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        }))
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(buffer_contains(
            &buffer,
            "tokens: 10 prompt + 5 completion = 15 total"
        ));
    }

    // --- history recall (ticket 05) ---------------------------------------

    #[test]
    fn up_at_empty_input_enters_recall_with_newest() {
        let mut app = seeded_app();
        app.set_history(vec!["older".to_string(), "newest".to_string()]);
        let effect = app.handle_key(key(KeyCode::Up));
        assert_eq!(effect, None);
        assert!(app.recall.is_some());
        // Recall starts at the newest entry (last in store order).
        assert_eq!(app.input_text(), "newest");
    }

    #[test]
    fn down_at_empty_input_also_enters_recall() {
        let mut app = seeded_app();
        app.set_history(vec!["only".to_string()]);
        app.handle_key(key(KeyCode::Down));
        assert!(app.recall.is_some());
        assert_eq!(app.input_text(), "only");
    }

    #[test]
    fn up_down_navigates_recall_newest_first() {
        let mut app = seeded_app();
        app.set_history(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        app.handle_key(key(KeyCode::Up)); // newest
        assert_eq!(app.input_text(), "c");
        app.handle_key(key(KeyCode::Up)); // older
        assert_eq!(app.input_text(), "b");
        app.handle_key(key(KeyCode::Up)); // older
        assert_eq!(app.input_text(), "a");
        app.handle_key(key(KeyCode::Up)); // clamp at the oldest
        assert_eq!(app.input_text(), "a");
        app.handle_key(key(KeyCode::Down)); // newer
        assert_eq!(app.input_text(), "b");
        app.handle_key(key(KeyCode::Down)); // newer
        assert_eq!(app.input_text(), "c");
        // ↓ at the newest entry exits recall back to empty editing.
        app.handle_key(key(KeyCode::Down));
        assert!(app.recall.is_none());
        assert_eq!(app.input_text(), "");
    }

    #[test]
    fn typing_exits_recall_and_keeps_text() {
        let mut app = seeded_app();
        app.set_history(vec!["recalled".to_string()]);
        app.handle_key(key(KeyCode::Up));
        assert!(app.recall.is_some());
        assert_eq!(app.input_text(), "recalled");
        // Typing exits recall; the key is processed normally at the cursor.
        app.handle_key(key(KeyCode::Char('!')));
        assert!(app.recall.is_none());
        assert_eq!(app.input_text(), "recalled!");
    }

    #[test]
    fn vertical_keys_on_nonempty_input_do_not_enter_recall() {
        let mut app = seeded_app();
        app.set_history(vec!["stored".to_string()]);
        type_text(&mut app, "abc");
        app.handle_key(key(KeyCode::Up));
        assert!(app.recall.is_none());
        assert_eq!(app.input_text(), "abc");
    }

    #[test]
    fn enter_in_recall_replays_without_recording() {
        let mut app = seeded_app();
        app.set_history(vec!["older".to_string(), "recent".to_string()]);
        // A fresh prompt is recorded into the snapshot.
        type_text(&mut app, "brand new");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::SubmitPrompt("brand new".to_string()))
        );
        assert_eq!(app.history, vec!["older", "recent", "brand new"]);
        // Recall the newest prompt and press Enter: it re-runs as a fresh
        // turn without being recorded again.
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.input_text(), "brand new");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::ReplayPrompt("brand new".to_string()))
        );
        assert_eq!(app.history, vec!["older", "recent", "brand new"]);
    }

    #[test]
    fn replay_commands_do_not_record_history() {
        let mut app = seeded_app();
        app.set_history(vec!["older".to_string(), "recent".to_string()]);
        type_text(&mut app, "/!!");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::ReplayHistory(1))
        );
        type_text(&mut app, "/!2");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::ReplayHistory(2))
        );
        // The `/!!` / `/!N` replay path never records into the snapshot.
        assert_eq!(app.history, vec!["older", "recent"]);
    }

    #[test]
    fn slash_history_lists_prompts_as_transcript_entries() {
        let mut app = seeded_app();
        type_text(&mut app, "/history");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::ListHistory)
        );
        // The loop fulfils the effect through the shared renderer.
        let entries = vec!["one".to_string(), "two".to_string()];
        let lines = render_history(&entries, HISTORY_DISPLAY);
        for line in lines {
            app.push_notice(line);
        }
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(buffer_contains(&buffer, "2: one"));
        assert!(buffer_contains(&buffer, "1: two"));
    }

    #[test]
    fn ctrl_c_quits_from_recall() {
        let mut app = seeded_app();
        app.set_history(vec!["stored".to_string()]);
        app.handle_key(key(KeyCode::Up));
        assert!(app.recall.is_some());
        assert_eq!(app.handle_key(ctrl_key('c')), Some(Effect::Quit));
    }
}
