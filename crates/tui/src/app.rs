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
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use slimcode_agent::agent::StopReason;
use slimcode_commands::{COMMANDS, find};
use slimcode_common::render::{DisplayItem, Renderer, usage_summary};
use slimcode_common::skills::{
    CompletionItem, Skill, SkillScope, combined_suggestions, complete, find_skill,
};
use tui_textarea::{CursorMove, TextArea};
use unicode_width::UnicodeWidthChar;

use crate::footer::FooterUsage;
use crate::markdown::render_markdown;
use crate::text::{display_width, wrap_to_width};
use crate::theme::{BgToken, Token, bg, fg};
use crate::toolcall::{CallPart, tool_call_title};

/// Resting height in rows of the input box (including its border): one content
/// row. The box grows with its (wrapped) content up to [`MAX_INPUT_RATIO`] of
/// the terminal height, so multi-line prompts stay visible while editing.
const INPUT_HEIGHT: u16 = 3;

/// Max share of the terminal height the input box may occupy (pi-style): a
/// 24-row terminal caps the input at ~7 rows.
const MAX_INPUT_RATIO: u16 = 30;

/// Maximum number of rows the `/` completion popup shows before scrolling.
const COMPLETION_VISIBLE: usize = 5;

/// Number of lines a PageUp / PageDown key scrolls the transcript by.
const PAGE_LINES: usize = 10;

/// Rows available to the footer (pi's two-line footer).
const FOOTER_HEIGHT: u16 = 2;

/// pi's braille spinner frames (`DEFAULT_FRAMES` in pi's `loader.ts`), shown
/// in the status indicator row while a turn runs.
const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The message next to the spinner (pi's `defaultWorkingMessage`).
const WORKING_MESSAGE: &str = "Working...";

/// Number of output lines a collapsed tool block shows before the expand hint.
const TOOL_PREVIEW_LINES: usize = 10;

/// Spinner/spacer frames before the transcript scrollbar fades out (auto
/// mode): 12 ticks ≈ 1s at the loop's 80ms frame interval.
const SCROLLBAR_FADE_TICKS: u8 = 12;

/// The startup header's single compact hint line (`·`-separated); full help
/// stays under `/help`.
const HEADER_HINTS: &str = "/help for commands · /skills to run · ↑ history · Ctrl+O expand";

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
    /// Best-effort git branch of the cwd (`None` outside a repository), fed
    /// from the shell at startup and on session changes.
    pub branch: Option<String>,
    /// Session-total token usage for the footer stats line. The shell feeds
    /// the provider's cumulative usage after each turn (and `/usage` keeps
    /// its own dim detail line).
    pub usage: FooterUsage,
}

/// Lifecycle state of a paired tool block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    /// The tool started; awaiting its result.
    Pending,
    /// The tool finished successfully.
    Success,
    /// The tool failed.
    Error,
}

/// One row-group in the transcript.
///
/// Blocks are pi-style display units: boxed user prompts, merged assistant
/// and thinking streams, paired tool blocks (start/result), and flat
/// frontend-owned notices/errors. The old flat glyph lines (turn markers,
/// `▶`/`✔`/`✖`, `✓ done`) are gone; everything renders through the Theme
/// tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// The startup header block (name + version + hints).
    Header,
    /// A boxed user prompt rendered as markdown (`userMessageBg`).
    UserPrompt { text: String },
    /// Merged streamed assistant text, rendered as markdown.
    Assistant { text: String },
    /// Merged streamed reasoning (thinking) text, italic gray markdown.
    Thinking { text: String },
    /// A paired tool start/result block: state-colored background, bold title,
    /// pretty args, gray output, collapsed to [`TOOL_PREVIEW_LINES`].
    Tool {
        name: String,
        args: String,
        output: String,
        status: ToolStatus,
    },
    /// Frontend-owned output (command results, notices): dim.
    Notice(String),
    /// An inline error (failed turn, failed command, abnormal stop): red.
    Error(String),
}

/// A side effect the terminal loop fulfills after the app processed a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Quit the TUI (Ctrl+C / Ctrl+D / `/exit`).
    Quit,
    /// Quit after the currently running turn finishes (Ctrl+C / Ctrl+D while
    /// a turn runs; the worker keeps streaming, keys are otherwise ignored
    /// until the turn ends, then the TUI exits).
    QuitAfterTurn,
    /// Cancel the running turn at the next runner boundary (Esc while a turn
    /// runs): the worker aborts its in-flight request / tool and returns to
    /// idle with whatever already streamed/applied kept.
    CancelRunning,
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

/// Active `/` completion state: the fuzzy candidate list shown below the
/// input box, the selected row (wrap-around), and the scroll window start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Ranked candidates (best first) for the current partial `/` input.
    pub items: Vec<CompletionItem>,
    /// Index of the highlighted candidate into `items`.
    pub selected: usize,
    /// First row of the scroll window into `items` (keeps the selection
    /// visible without scrolling every keystroke).
    pub offset: usize,
}

impl Completion {
    /// Clamp the scroll window so the selection stays visible.
    fn clamp_offset(&mut self) {
        if self.items.is_empty() {
            self.selected = 0;
            self.offset = 0;
            return;
        }
        let max = self.items.len().saturating_sub(1);
        self.selected = self.selected.min(max);
        let window = COMPLETION_VISIBLE.min(self.items.len());
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + window {
            self.offset = self.selected + 1 - window;
        }
    }
}

/// The pure TUI app core.
pub struct App {
    /// The transcript rendered in the top pane.
    pub transcript: Vec<Entry>,
    /// The multi-line input box.
    pub input: TextArea<'static>,
    /// Status-line state.
    pub status: StatusLine,
    /// Snapshot of installed skills used for `/skill` dispatch and suggestions;
    /// refreshed after a runtime `/install-skill` (see [`App::set_skills`]).
    pub skills: Vec<Skill>,
    /// Recent prompts for ↑/↓ recall, in `HistoryStore` order (oldest first,
    /// newest last — recall starts at the newest entry, matching `/!1`). The
    /// terminal loop seeds this from `HistoryStore` at startup; fresh prompts
    /// are appended here on submit, mirroring `HistoryStore::append`.
    pub history: Vec<String>,
    /// Active history-recall state, if any.
    pub recall: Option<Recall>,
    /// Active `/` completion popup state, if any.
    pub completion: Option<Completion>,
    /// Lines scrolled up from the bottom of the transcript (0 = at bottom).
    pub scroll: usize,
    /// Whether the view auto-follows new output.
    pub follow: bool,
    /// The transcript content width (pane width minus its borders) from the
    /// most recent draw; 0 before the first draw. Used to wrap long lines so
    /// scroll/window row math matches what is rendered.
    content_width: u16,
    /// Version string shown in the startup header (slimcode's version).
    version: String,
    /// Global tool-output expansion flag: Ctrl+O expands every tool block.
    tool_output_expanded: bool,
    /// Fade counter for the auto-mode transcript scrollbar; decremented by
    /// [`App::tick`].
    scrollbar_ticks: u8,
    /// Index into [`SPINNER_FRAMES`] for the status indicator; advanced every
    /// [`App::tick`] while running (~80ms per frame, pi's loader interval).
    spinner_frame: usize,
}

impl App {
    /// Create a fresh app in "ready" state. `version` feeds the startup
    /// header block.
    pub fn new(
        cwd: impl Into<String>,
        session_id: impl Into<String>,
        model: impl Into<String>,
        version: impl Into<String>,
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
                branch: None,
                usage: FooterUsage::default(),
            },
            skills,
            history: Vec::new(),
            recall: None,
            completion: None,
            scroll: 0,
            follow: true,
            content_width: 0,
            version: version.into(),
            tool_output_expanded: false,
            scrollbar_ticks: 0,
            spinner_frame: 0,
        };
        app.transcript.push(Entry::Header);
        app
    }

    /// The current input text (joined across lines).
    pub fn input_text(&self) -> String {
        self.input.lines().join("\n")
    }

    /// Mark the app as running (or not) on the status line.
    pub fn set_running(&mut self, running: bool) {
        self.status.running = running;
        if !running {
            // Idle: the indicator hides, so the frame index can restart.
            self.spinner_frame = 0;
        }
    }

    /// Feed the best-effort git branch (shell reads it at startup and on
    /// session changes). `None` outside a repository.
    pub fn set_branch(&mut self, branch: Option<String>) {
        self.status.branch = branch;
    }

    /// Feed the session-total token usage for the footer stats line (the
    /// provider's cumulative totals after each turn).
    pub fn set_usage(&mut self, usage: FooterUsage) {
        self.status.usage = usage;
    }

    /// The current spinner frame character (pi braille loader).
    fn spinner_char(&self) -> char {
        SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()]
    }

    /// Key handling while a turn runs on the worker thread (ticket 07 R3):
    /// bare Esc cancels the running turn ([`Effect::CancelRunning`]);
    /// Ctrl+C / Ctrl+D set a quit-after-turn flag (the current turn keeps
    /// streaming to completion); every other key is ignored. Called by the
    /// shell's running loop instead of [`App::handle_key`].
    pub fn handle_key_running(&mut self, key: KeyEvent) -> Option<Effect> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('d') => return Some(Effect::QuitAfterTurn),
                _ => {}
            }
        }
        // Bare Esc cancels the run; modified variants (Ctrl/Alt/Shift+Esc)
        // are not cancels.
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            return Some(Effect::CancelRunning);
        }
        None
    }

    /// Append frontend-owned output to the transcript (dim notice).
    pub fn push_notice(&mut self, text: impl Into<String>) {
        self.transcript.push(Entry::Notice(text.into()));
        self.reset_view();
    }

    /// Append an inline error entry to the transcript (red).
    pub fn push_error(&mut self, text: impl Into<String>) {
        self.transcript.push(Entry::Error(text.into()));
        self.reset_view();
    }

    /// Append a boxed user prompt block (typed, recalled, or `/!!`-replayed
    /// prompts all render the same way; the `> prompt` notice line is gone).
    pub fn push_user_prompt(&mut self, text: impl Into<String>) {
        self.transcript
            .push(Entry::UserPrompt { text: text.into() });
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

    /// Replace the skills snapshot. The terminal loop calls this after a
    /// runtime `/install-skill` re-reads the store, so `/` completion,
    /// did-you-mean prediction, and skill dispatch see the new skill
    /// immediately instead of on the next launch.
    pub fn set_skills(&mut self, skills: Vec<Skill>) {
        self.skills = skills;
    }

    /// On-key reducer: returns the effect (if any) the loop must fulfil.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Effect> {
        // Global control keys first: Ctrl+C / Ctrl+D quit from any state;
        // Ctrl+O toggles tool-output expansion (pi `toolOutputExpanded`).
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('d') => return Some(Effect::Quit),
                KeyCode::Char('o') => {
                    self.tool_output_expanded = !self.tool_output_expanded;
                    return None;
                }
                _ => {}
            }
        }
        let completion_open = self.completion.is_some();
        match key.code {
            // ↑/↓ with the completion popup open navigate it (wrap-around);
            // otherwise they enter history recall at the empty input or move
            // the text-area cursor.
            KeyCode::Up | KeyCode::Down => {
                if completion_open {
                    self.completion_arrow(key.code);
                } else {
                    self.handle_vertical(key);
                }
                None
            }
            // Enter with the popup open applies the selected completion and
            // submits it (execute); otherwise recall/newline/submit as before.
            KeyCode::Enter => {
                if completion_open && !key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.accept_completion_and_submit()
                } else {
                    let effect = self.handle_enter(key);
                    self.refresh_completion();
                    effect
                }
            }
            // Tab accepts the selected completion onto the buffer with a
            // trailing space (no submit); with `/` typed but no popup yet it
            // force-opens the popup; otherwise the text area handles it.
            KeyCode::Tab => {
                if completion_open {
                    self.accept_completion();
                } else if self.input_text().starts_with('/') {
                    self.refresh_completion();
                } else {
                    self.input.input(key);
                }
                None
            }
            // Escape cancels the popup (keeps the typed text) and recall.
            KeyCode::Esc => {
                self.recall = None;
                self.completion = None;
                None
            }
            // PageUp/PageDown page the completion list while it is open;
            // otherwise they scroll the transcript.
            KeyCode::PageUp => {
                if completion_open {
                    self.completion_page(PAGE_LINES, true);
                } else {
                    self.scroll_up(PAGE_LINES);
                }
                None
            }
            KeyCode::PageDown => {
                if completion_open {
                    self.completion_page(PAGE_LINES, false);
                } else {
                    self.scroll_down(PAGE_LINES);
                }
                None
            }
            // Everything else exits recall (if active), goes to the text area
            // (typing, arrows, ...), and recomputes the completion popup.
            _ => {
                self.recall = None;
                // pi's newline key: Ctrl+J inserts a newline (tui_textarea
                // would otherwise treat it as delete-line-by-head).
                if key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.input.insert_newline();
                } else {
                    self.input.input(key);
                }
                self.refresh_completion();
                None
            }
        }
    }

    /// Whether the `/` completion popup should currently be active: the whole
    /// input is a single line, it starts with `/`, and no argument whitespace
    /// has been typed yet (a space closes the popup so the user can type
    /// arguments after the completed command name).
    fn completion_active(&self) -> bool {
        self.input.lines().len() == 1
            && self.input_text().starts_with('/')
            && !self.input_text()[1..].contains(char::is_whitespace)
    }

    /// Recompute the candidate list from the current input. Preserves the
    /// selected value when it is still a candidate; otherwise selects the best
    /// match (first). Closes the popup when the input leaves the `/` context
    /// or nothing matches.
    fn refresh_completion(&mut self) {
        if !self.completion_active() {
            self.completion = None;
            return;
        }
        let items = complete(&self.input_text(), &self.skills);
        if items.is_empty() {
            self.completion = None;
            return;
        }
        let kept = self
            .completion
            .as_ref()
            .and_then(|c| c.items.get(c.selected).map(|i| i.value.clone()));
        let selected = kept
            .and_then(|value| items.iter().position(|i| i.value == value))
            .unwrap_or(0);
        let mut comp = Completion {
            items,
            selected,
            offset: 0,
        };
        comp.clamp_offset();
        self.completion = Some(comp);
    }

    /// Move the completion selection, wrapping around the ends.
    fn completion_arrow(&mut self, code: KeyCode) {
        let Some(comp) = self.completion.as_mut() else {
            return;
        };
        let len = comp.items.len();
        if len == 0 {
            return;
        }
        match code {
            KeyCode::Up => comp.selected = (comp.selected + len - 1) % len,
            KeyCode::Down => comp.selected = (comp.selected + 1) % len,
            _ => {}
        }
        comp.clamp_offset();
    }

    /// Move the completion selection by a page of `lines`, saturating at the
    /// ends (no wrap-around for pages).
    fn completion_page(&mut self, lines: usize, up: bool) {
        let Some(comp) = self.completion.as_mut() else {
            return;
        };
        let max = comp.items.len().saturating_sub(1);
        if up {
            comp.selected = comp.selected.saturating_sub(lines);
        } else {
            comp.selected = (comp.selected + lines).min(max);
        }
        comp.clamp_offset();
    }

    /// Accept the selected completion onto the input buffer with a trailing
    /// space (cursor lands after it) and close the popup, without submitting.
    /// No-op when there is no active popup or nothing is selected.
    fn accept_completion(&mut self) {
        let Some(comp) = self.completion.take() else {
            return;
        };
        if let Some(item) = comp.items.get(comp.selected) {
            self.set_input_text(&format!("{} ", item.value));
        }
    }

    /// Apply the selected completion to the buffer (no trailing space) and
    /// submit it as a command: Enter executes the selection, not the literal
    /// partial text. Falls back to a plain submit when there is no popup.
    fn accept_completion_and_submit(&mut self) -> Option<Effect> {
        let Some(comp) = self.completion.take() else {
            return self.submit_current_input();
        };
        if let Some(item) = comp.items.get(comp.selected).cloned() {
            self.set_input_text(&item.value);
        }
        self.submit_current_input()
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
            self.push_user_prompt(text.clone());
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
        // The completion popup (when open) is an extra region between the
        // input box and the footer: a bordered list of `visible` rows plus a
        // muted `(i/n)` scroll-info row when the list overflows.
        let completion = self.completion.as_ref();
        let visible = completion
            .map(|c| c.items.len().min(COMPLETION_VISIBLE))
            .unwrap_or(0);
        let scroll_info = completion.map(|c| c.items.len() > visible).unwrap_or(false);
        // Borderless SelectList popup (ticket 02) with a full-width top
        // separator line (ticket 05): one row per visible candidate plus the
        // `(i/n)` overflow row when it overflows, plus 1 separator row. The
        // separator keeps a clear visual gap between the transcript and the
        // popup (which sits directly above the input box). No box border.
        let popup_height = if completion.is_some() && visible > 0 {
            (visible + usize::from(scroll_info)) as u16 + 1
        } else {
            0
        };

        // Word-wrap the input at the box's content width and let the box grow
        // with its content (up to 30% of the terminal), so multi-line prompts
        // stay visible while editing (pi-style).
        let input_content_width = (area.width.saturating_sub(2)).max(1) as usize;
        let (input_rows, cursor) = wrapped_input(&self.input, input_content_width);
        let input_height = input_box_height(area.height, input_rows.len());
        let running = self.status.running;

        // Ticket-02 dock: transcript | popup | editor | footer. The popup
        // (0 rows when closed) sits directly above the editor so the editor
        // and footer stay anchored and the input box never shifts when the
        // popup opens/closes (the popup eats into the transcript instead). The
        // runner status is embedded in the editor's top border while running.
        let [transcript_area, popup_area, input_area, footer_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(popup_height),
            Constraint::Length(input_height),
            Constraint::Length(FOOTER_HEIGHT),
        ])
        .areas(area);

        // Transcript pane: full-bleed, windowed to the pane height; block
        // rows are already width-fitted (user/tool boxes pad to the width,
        // markdown wraps), so nothing is truncated.
        let content_width = transcript_area.width;
        self.content_width = content_width;
        let lines = self.visible_lines(transcript_area.height, content_width);
        frame.render_widget(Paragraph::new(lines), transcript_area);
        self.render_scrollbar(frame, transcript_area, self.total_lines());

        // Input box: word-wrapped rows, dynamic height, cursor kept visible.
        // The runner status is drawn into the box's top border while running.
        render_input(
            frame,
            input_area,
            &self.input,
            &input_rows,
            cursor,
            running,
            self.spinner_char(),
        );

        // Completion popup, when active (pi SelectList style, borderless,
        // above the input box — ticket 02).
        if let Some(comp) = &self.completion
            && popup_height > 0
        {
            self.render_completion(frame, popup_area, comp);
        }

        // Status indicator: the runner status is embedded in the input box's
        // top border (render_input) while a turn runs; there is no separate
        // status row.
        self.render_footer(frame, footer_area);
    }

    /// Render the pi-style two-line dock footer (ADR-0006 D5): line 1 = dim
    /// `~/path (branch) • session`, line 2 = dim stats with the model
    /// right-aligned; both truncated to the pane width. Composed from pure
    /// footer-formatting functions so the layout is unit-testable.
    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        if area.is_empty() || area.width < 2 {
            return;
        }
        let width = area.width as usize;
        let home = std::env::var("HOME").ok();
        let pwd = crate::footer::format_cwd_for_footer(&self.status.cwd, home.as_deref());
        let mut pwd = pwd;
        if let Some(branch) = &self.status.branch {
            pwd = format!("{pwd} ({branch})");
        }
        pwd = format!("{} • {}", pwd, self.status.session_id);
        let line1 = crate::footer::truncate_width_str(&pwd, width);

        let stats = crate::footer::stats_parts(&self.status.usage);
        let line2 = crate::footer::stats_line(&stats, &self.status.model, width);

        let rows = vec![
            Line::styled(line1, fg(Token::Dim)),
            Line::styled(line2, fg(Token::Dim)),
        ];
        frame.render_widget(Paragraph::new(rows), area);
    }

    /// Render the `/` completion popup into `area`: a full-width top separator
    /// line (`─`, `Token::Border`) followed by bare SelectList rows (pi style,
    /// no box, no title) — selected row `→ ` prefix + name in `accent`,
    /// non-selected rows default, descriptions `muted`, and a muted `(i/n)`
    /// overflow row when the list overflows [`COMPLETION_VISIBLE`] (tickets
    /// 02 + 05). Rendered above the input box so the input position stays
    /// stable; the separator keeps a visual gap from the transcript.
    fn render_completion(&self, frame: &mut Frame, area: Rect, comp: &Completion) {
        let visible = comp.items.len().min(COMPLETION_VISIBLE);
        let start = comp.offset;
        let end = (comp.offset + visible).min(comp.items.len());
        let window: &[CompletionItem] = &comp.items[start..end];

        let name_width = window
            .iter()
            .map(|i| i.value.chars().count())
            .max()
            .unwrap_or(0);
        let mut rows: Vec<Line> =
            Vec::with_capacity(window.len() + usize::from(comp.items.len() > visible) + 1);
        // Top separator: full-width `─` in the semantic border color, so the
        // popup reads as a distinct region from the transcript above it.
        rows.push(Line::from(vec![Span::styled(
            "─".repeat(area.width as usize),
            fg(Token::Border),
        )]));
        for (i, item) in window.iter().enumerate() {
            let selected = (start + i) == comp.selected;
            let prefix = if selected { "→ " } else { "  " };
            let name = format!("{prefix}{:<width$}", item.value, width = name_width);
            // pi SelectList: selected prefix + text are `accent`, no bold, no
            // background inversion.
            let name_style = if selected {
                fg(Token::Accent)
            } else {
                Style::default()
            };
            let mut spans = vec![Span::styled(name, name_style)];
            if !item.description.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", item.description),
                    fg(Token::Muted),
                ));
            }
            rows.push(Line::from(spans));
        }

        let total = comp.items.len();
        if total > visible {
            // SelectList scroll info `(i/n)`, muted (its own row, last).
            rows.push(Line::styled(
                format!("  ({}/{total})", comp.selected + 1),
                fg(Token::Muted),
            ));
        }
        frame.render_widget(Paragraph::new(rows), area);
    }

    /// Push a streamed display item into the transcript and re-follow.
    ///
    /// Consecutive streamed text (and reasoning) fragments merge into a
    /// single assistant/thinking block, so a multi-delta stream renders as one
    /// flowing block. Tool start/result pair into one block (sequential per
    /// the shared runner); turn markers and the `✓ done` stop line are gone.
    fn push_display_item(&mut self, item: DisplayItem) {
        match item {
            // Turn markers were removed in the pi alignment (no `── turn N ──`).
            DisplayItem::Turn { .. } => return,
            DisplayItem::Text(fragment) => {
                if let Some(Entry::Assistant { text }) = self.transcript.last_mut() {
                    text.push_str(&fragment);
                } else {
                    self.transcript.push(Entry::Assistant { text: fragment });
                }
            }
            DisplayItem::Reasoning(fragment) => {
                if let Some(Entry::Thinking { text }) = self.transcript.last_mut() {
                    text.push_str(&fragment);
                } else {
                    self.transcript.push(Entry::Thinking { text: fragment });
                }
            }
            DisplayItem::ToolStart { name, arguments } => {
                self.transcript.push(Entry::Tool {
                    name,
                    args: arguments,
                    output: String::new(),
                    status: ToolStatus::Pending,
                });
            }
            DisplayItem::ToolResult { name, ok, result } => {
                let status = if ok {
                    ToolStatus::Success
                } else {
                    ToolStatus::Error
                };
                // Pair with the last pending block for the same tool (the
                // shared runner emits start/result sequentially per tool).
                let pending = self.transcript.iter().rposition(|entry| {
                    matches!(
                        entry,
                        Entry::Tool {
                            name: n,
                            status: ToolStatus::Pending,
                            ..
                        } if *n == name
                    )
                });
                if let Some(idx) = pending {
                    if let Entry::Tool {
                        output, status: st, ..
                    } = &mut self.transcript[idx]
                    {
                        output.push_str(&result);
                        *st = status;
                    }
                } else {
                    self.transcript.push(Entry::Tool {
                        name,
                        args: String::new(),
                        output: result,
                        status,
                    });
                }
            }
            // A completed run renders nothing (no `✓ done` line); a cancelled
            // run (Esc) also renders nothing — the partial transcript is the
            // feedback. There is no abnormal stop: a run ends either because
            // the model stopped calling tools or because the user cancelled.
            DisplayItem::Stop(StopReason::Completed) => return,
            DisplayItem::Stop(StopReason::Cancelled) => return,
            // `/usage` and the CLI summary share the dim notice line; the
            // per-turn usage line is gone (the footer shows totals, ticket 03).
            DisplayItem::Usage(u) => {
                self.transcript.push(Entry::Notice(usage_summary(&u)));
            }
        }
        self.reset_view();
    }

    /// One loop frame (~80ms): decrement the scrollbar fade counter (auto
    /// mode) and advance the spinner frame while a turn runs (pi's loader
    /// ticks at 80ms). The terminal loop calls this every frame, so the
    /// transcript scrollbar fades out ~1s after the last scroll gesture and
    /// the status spinner animates while running.
    pub fn tick(&mut self) {
        if self.scrollbar_ticks > 0 {
            self.scrollbar_ticks -= 1;
        }
        if self.status.running {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
        }
    }

    /// Re-anchor the view at the bottom (follow mode): used after new content
    /// is appended and after a transcript replacement.
    fn reset_view(&mut self) {
        self.scroll = 0;
        self.follow = true;
        self.scrollbar_ticks = 0;
    }

    /// Scroll the transcript up `lines` and stop following.
    fn scroll_up(&mut self, lines: usize) {
        self.follow = false;
        self.scrollbar_ticks = SCROLLBAR_FADE_TICKS;
        let total = self.total_lines();
        let max_scroll = total.saturating_sub(1);
        self.scroll = (self.scroll + lines).min(max_scroll);
    }

    /// Scroll the transcript down `lines`; reaching the bottom re-follows.
    fn scroll_down(&mut self, lines: usize) {
        self.follow = false;
        self.scrollbar_ticks = SCROLLBAR_FADE_TICKS;
        self.scroll = self.scroll.saturating_sub(lines);
        if self.scroll == 0 {
            self.follow = true;
        }
    }

    /// Number of transcript rows across all entries at the current content
    /// width (falling back to a small width estimate before the first draw).
    fn total_lines(&self) -> usize {
        let width = self.content_width.max(1) as usize;
        self.all_rows(width).len()
    }

    /// The window of transcript rows visible in a pane of `height` rows and
    /// `width` columns. Block rows are already width-fitted (user/tool boxes
    /// pad to the full width, markdown wraps), so every returned row fits the
    /// pane and no content is truncated.
    fn visible_lines(&self, height: u16, width: u16) -> Vec<Line<'static>> {
        let height = height as usize;
        let total = self.total_lines();
        if total == 0 || height == 0 {
            return Vec::new();
        }
        let max_scroll = total.saturating_sub(1);
        let scroll = self.scroll.min(max_scroll);
        let end = total.saturating_sub(scroll);
        let start = end.saturating_sub(height);

        let all = self.all_rows(width as usize);
        all[start..end].to_vec()
    }

    /// Build every transcript row once (used by both the window and the
    /// scrollbar thumb math), including the leading spacer row before user
    /// and tool blocks.
    fn all_rows(&self, width: usize) -> Vec<Line<'static>> {
        let mut rows: Vec<Line<'static>> = Vec::new();
        for entry in &self.transcript {
            let block = entry_rows(entry, width, self.tool_output_expanded, &self.version);
            if !block.is_empty()
                && !rows.is_empty()
                && matches!(entry, Entry::UserPrompt { .. } | Entry::Tool { .. })
            {
                rows.push(Line::default());
            }
            rows.extend(block);
        }
        rows
    }

    /// Whether the transcript scrollbar should render: pi ScrollView auto
    /// mode — the thumb appears after a scroll gesture and fades out after
    /// [`SCROLLBAR_FADE_TICKS`] frames (even while the view stays scrolled).
    fn scrollbar_showing(&self) -> bool {
        self.scrollbar_ticks > 0
    }

    /// Draw the auto-mode right-edge scrollbar thumb (`selectedBg`), over the
    /// rightmost content column (pi ScrollView overlay).
    fn render_scrollbar(&self, frame: &mut Frame, area: Rect, total: usize) {
        if !self.scrollbar_showing() || total <= area.height as usize {
            return;
        }
        let view = area.height as usize;
        let max_offset = total - view;
        let (start, _) = self.viewport(total, view);
        let thumb = (view * view / total).max(1).min(view);
        let band = view - thumb;
        // band == 0 exactly when max_offset == 0, so `max(1)` makes the
        // division safe without an explicit zero-check (clippy: avoid manual
        // checked division).
        let top = start * band / max_offset.max(1);
        for y in top..(top + thumb).min(view) {
            let cell = frame
                .buffer_mut()
                .cell_mut((area.right().saturating_sub(1), area.top() + y as u16));
            if let Some(cell) = cell {
                cell.set_symbol(" ");
                cell.set_style(bg(BgToken::SelectedBg));
            }
        }
    }

    /// The (start, end) row window for the current scroll position.
    fn viewport(&self, total: usize, view: usize) -> (usize, usize) {
        let max_scroll = total.saturating_sub(1);
        let scroll = self.scroll.min(max_scroll);
        let end = total.saturating_sub(scroll);
        let start = end.saturating_sub(view);
        (start, end)
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
            self.push_user_prompt(trimmed.to_string());
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
        // Skill trigger: `/skill-name` or the canonical `/skill:name`.
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
        self.push_notice("skills: /skills lists installed skills; /skill:<name> runs one");
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
                    "  /skill:{:<width$}  {}{}  [{}]",
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

/// Shared input-box decoration: placeholder only. The block (border style)
/// is rebuilt per frame in [`render_input`], so the border color can reflect
/// the running state (ADR-0006 D4); the ` input ` title is gone.
fn decorate_input(textarea: &mut TextArea<'static>) {
    textarea.set_placeholder_text("prompt… Enter submits, Shift+Enter newline, ↑ history");
}

/// Height of the input box (including its border): the wrapped content height
/// plus the border, grown with content but capped at [`MAX_INPUT_RATIO`] of the
/// terminal height (pi-style). At least the resting [`INPUT_HEIGHT`], and never
/// so tall that the footer is pushed off screen.
fn input_box_height(area_height: u16, wrapped_rows: usize) -> u16 {
    let desired = (wrapped_rows as u16).saturating_add(2); // + border
    let max = (area_height.saturating_mul(MAX_INPUT_RATIO) / 100)
        .max(INPUT_HEIGHT)
        .min(area_height.saturating_sub(FOOTER_HEIGHT));
    desired.clamp(INPUT_HEIGHT, max.max(INPUT_HEIGHT))
}

/// Word-wrap the input's logical lines at `width`, returning the visual rows
/// and the cursor's visual (row, col) within them. Every character of the
/// input appears exactly once across the rows (`wrap_to_width` never drops
/// chars), so the cursor column maps exactly. Returns `None` for the cursor
/// when the input is empty (the placeholder shows).
fn wrapped_input(input: &TextArea<'static>, width: usize) -> (Vec<String>, Option<(usize, usize)>) {
    let (cursor_line, cursor_col) = input.cursor();
    let mut rows: Vec<String> = Vec::new();
    let mut cursor = None;
    for (li, line) in input.lines().iter().enumerate() {
        let wrapped = wrap_to_width(line, width);
        let row_start = rows.len();
        rows.extend(wrapped);
        if li == cursor_line {
            let mut consumed = 0usize;
            for (ri, row) in rows[row_start..].iter().enumerate() {
                let n = row.chars().count();
                if cursor_col <= consumed + n {
                    cursor = Some((row_start + ri, cursor_col.saturating_sub(consumed)));
                    break;
                }
                consumed += n;
            }
            if cursor.is_none() {
                // Clamp to the last row of this logical line.
                let last = rows.len().saturating_sub(1);
                cursor = Some((last, rows[last].chars().count()));
            }
        }
    }
    (rows, cursor)
}

/// Render the input box: word-wrapped rows in a bordered block, scrolled so
/// the cursor row stays visible when the content overflows, with the terminal
/// cursor placed at the mapped position (pi-style editor behaviour).
fn render_input(
    frame: &mut Frame,
    area: Rect,
    input: &TextArea<'static>,
    rows: &[String],
    cursor: Option<(usize, usize)>,
    running: bool,
    spinner: char,
) {
    let width = area.width as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    // Keep the cursor row visible when the content is taller than the box.
    let top = match cursor {
        Some((row, _)) if row >= inner_h => row + 1 - inner_h,
        _ => 0,
    };
    let window: Vec<String> = rows.iter().skip(top).take(inner_h).cloned().collect();

    // Semantic border color: `border` blue at rest, `borderAccent` cyan while
    // a turn runs (ADR-0006 D4). The whole status+border row shares it.
    let border_style = fg(if running {
        Token::BorderAccent
    } else {
        Token::Border
    });

    // Borderless-sides editor (ticket 01, pi editor): full-width top and
    // bottom `─` lines with no corners and no vertical sides. While a turn
    // runs the top border embeds the runner status, left-aligned, matching
    // pi's embedWorkingStatus (`── ⠋ Working... ────`).
    let status: String = if running {
        format!("── {spinner} {WORKING_MESSAGE} ")
    } else {
        String::new()
    };
    let status_width = display_width(status.as_str());
    let top_fill = "─".repeat(width.saturating_sub(status_width));
    let mut lines: Vec<Line> = Vec::with_capacity(window.len() + 2);
    lines.push(Line::from(vec![Span::styled(
        format!("{status}{top_fill}"),
        border_style,
    )]));

    let content_width = width.saturating_sub(1); // 1-space left inset
    if input.is_empty() {
        // Placeholder: a cursor-width space plus the dim placeholder text.
        let placeholder = input.placeholder_text().to_string();
        let pad = content_width.saturating_sub(1 + display_width(&placeholder));
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(placeholder, Style::default().fg(Color::DarkGray)),
            Span::raw(" ".repeat(pad)),
        ]));
    } else {
        for row in &window {
            let row_width = display_width(row);
            let pad = content_width.saturating_sub(row_width);
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::raw(row.clone()),
                Span::raw(" ".repeat(pad)),
            ]));
        }
    }

    lines.push(Line::from(vec![Span::styled(
        "─".repeat(width),
        border_style,
    )]));
    frame.render_widget(Paragraph::new(lines), area);

    // Place the terminal cursor at the input cursor's visual position.
    if let Some((row, col)) = cursor {
        let visual_row = row.saturating_sub(top);
        let prefix: String = rows[row].chars().take(col).collect();
        let x = area.x + 1 + display_width(prefix.as_str()) as u16;
        let y = area.y + 1 + visual_row as u16;
        if y < area.y + area.height && x < area.x + area.width {
            frame.set_cursor_position(Position { x, y });
        }
    }
}

/// Split a `/command` line into its name (leading slash kept) and optional
/// argument.
fn split_name_arg(line: &str) -> (&str, Option<&str>) {
    let mut parts = line.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let arg = parts.next().filter(|a| !a.trim().is_empty());
    (name, arg)
}

/// The styled rows an entry contributes to the transcript (block-aware,
/// already width-fitted; see ADR-0006 D2/D4).
#[allow(clippy::too_many_arguments)]
fn entry_rows(
    entry: &Entry,
    width: usize,
    tool_expanded: bool,
    version: &str,
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    match entry {
        Entry::Header => header_rows(version),
        Entry::UserPrompt { text } => user_box_rows(text, width),
        Entry::Assistant { text } => render_markdown(text, width),
        Entry::Thinking { text } => render_markdown(text, width)
            .into_iter()
            .map(|line| italicize(line, Token::ThinkingText.color()))
            .collect(),
        Entry::Tool {
            name,
            args,
            output,
            status,
        } => tool_rows(name, args, output, *status, width, tool_expanded),
        Entry::Notice(text) => text
            .lines()
            .map(|l| Line::styled(l.to_string(), fg(Token::Dim)))
            .collect(),
        Entry::Error(text) => text
            .lines()
            .map(|l| Line::styled(l.to_string(), fg(Token::Error)))
            .collect(),
    }
}

/// The startup header block: bold accent `slimcode` + dim ` v<version>` + one
/// compact hint line.
fn header_rows(version: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("slimcode", fg(Token::Accent).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" v{version}"),
                fg(Token::Dim).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::styled(HEADER_HINTS, fg(Token::Dim)),
    ]
}

/// A boxed user prompt: `userMessageBg`-filled full-width rows with the
/// markdown text padded one column each side (pi Box paddingX=1).
fn user_box_rows(text: &str, width: usize) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(2).max(1);
    let md = render_markdown(text, inner);
    let bg = BgToken::UserMessageBg.color();
    let bg_style = Style::default().bg(bg);
    let mut rows = vec![Line::styled(" ".repeat(width), bg_style)];
    for line in md {
        let mut spans: Vec<Span> = line
            .spans
            .into_iter()
            .map(|s| Span::styled(s.content.clone(), s.style.bg(bg)))
            .collect();
        let content_w: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        let pad = inner.saturating_sub(content_w);
        spans.insert(0, Span::styled(" ", bg_style));
        spans.push(Span::styled(" ".repeat(pad + 1), bg_style));
        rows.push(Line::from(spans));
    }
    rows.push(Line::styled(" ".repeat(width), bg_style));
    rows
}

/// A tool block: state-colored full-width background, compact per-tool call
/// title for built-ins (pi `format*Call`; ticket 06), gray output collapsed to
/// [`TOOL_PREVIEW_LINES`] with an expand hint unless the global Ctrl+O
/// expansion flag is on.
///
/// Every non-spacer row is padded to the pane width so the state background
/// reads as one solid full-width band (ticket 05; pi Box paints the whole
/// padded rect). Spacer rows stay `" ".repeat(width)`. Unknown tools (no
/// compact shape) keep pi's fallback header: bold bare name + pretty JSON
/// args.
fn tool_rows(
    name: &str,
    args: &str,
    output: &str,
    status: ToolStatus,
    width: usize,
    expanded: bool,
) -> Vec<Line<'static>> {
    let bg = match status {
        ToolStatus::Pending => BgToken::ToolPendingBg.color(),
        ToolStatus::Success => BgToken::ToolSuccessBg.color(),
        ToolStatus::Error => BgToken::ToolErrorBg.color(),
    };
    let bg_style = Style::default().bg(bg);
    let inner = width.saturating_sub(2).max(1);
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.push(Line::styled(" ".repeat(width), bg_style));

    match tool_call_title(name, args) {
        // Built-in tool with a compact call-title shape: wrapped title rows
        // (no separate JSON args section).
        Some(parts) => {
            for spans in wrap_call_title(&parts, bg_style, inner) {
                let mut row = vec![Span::styled(" ", bg_style)];
                row.extend(spans);
                rows.push(pad_line_to_width(row, width, bg_style));
            }
        }
        // Unknown tool / unusable args → pi fallback: bold bare name, blank
        // line, pretty JSON args (unchanged layout).
        None => {
            let title = vec![
                Span::styled(" ", bg_style),
                Span::styled(
                    name.to_string(),
                    bg_style
                        .fg(Token::ToolTitle.color())
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            rows.push(pad_line_to_width(title, width, bg_style));
            let pretty = pretty_args(args);
            if !pretty.is_empty() {
                rows.push(Line::styled(" ".repeat(width), bg_style));
                for line in pretty {
                    let style = bg_style.fg(Token::Dim.color());
                    rows.push(pad_line_to_width(
                        vec![Span::styled(
                            format!(" {}", wrap_to_width_join(&line, inner)),
                            style,
                        )],
                        width,
                        bg_style,
                    ));
                }
            }
        }
    }

    // Output: gray (toolOutput), collapsed to the first N lines.
    let out_lines: Vec<String> = output
        .lines()
        .flat_map(|l| wrap_to_width(l, inner))
        .collect();
    let remaining = out_lines.len().saturating_sub(TOOL_PREVIEW_LINES);
    let visible_lines = if expanded || remaining == 0 {
        &out_lines[..]
    } else {
        &out_lines[..TOOL_PREVIEW_LINES]
    };
    for line in visible_lines {
        let style = bg_style.fg(Token::ToolOutput.color());
        rows.push(pad_line_to_width(
            vec![Span::styled(format!(" {line}"), style)],
            width,
            bg_style,
        ));
    }
    if remaining > 0 && !expanded {
        // pi tool-execution collapse hint: muted prefix + dim key hint +
        // muted suffix (the same text parts as keyHint renders).
        let hint_bg = bg_style.fg(Token::Muted.color());
        let hint = vec![
            Span::styled(format!(" ... ({} more lines,", remaining), hint_bg),
            Span::styled(" Ctrl+O", bg_style.fg(Token::Dim.color())),
            Span::styled(" to expand)", hint_bg),
        ];
        rows.push(pad_line_to_width(hint, width, bg_style));
    }
    rows.push(Line::styled(" ".repeat(width), bg_style));
    rows
}

/// Wrap a tool-call title's styled runs to `width` display cells on the block
/// background `bg`. Breaks at spaces when possible (words stay whole) and at
/// character boundaries otherwise, so no title text is dropped and wide CJK
/// characters count double (pi's Text component wraps the same way). Each
/// returned row is the styled title spans without the leading padding column.
fn wrap_call_title(parts: &[CallPart], bg: Style, width: usize) -> Vec<Vec<Span<'static>>> {
    let style_for = |part: &CallPart| {
        let mut style = bg.fg(part.fg.color());
        if part.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    };
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut row_widths = vec![0usize];
    for part in parts {
        let style = style_for(part);
        let mut rest = part.text.as_str();
        while !rest.is_empty() {
            let used = *row_widths.last().unwrap();
            if used > 0 && used >= width {
                rows.push(Vec::new());
                row_widths.push(0);
                continue;
            }
            let mut chunk = String::new();
            let mut chunk_w = 0usize;
            let mut consumed = 0usize;
            let mut word_break = false;
            for (i, c) in rest.char_indices() {
                let cw = c.width().unwrap_or(0);
                consumed = i + c.len_utf8();
                if !chunk.is_empty() && chunk_w + cw > width {
                    // Prefer a word boundary: cut at the last space so words
                    // stay whole; overlong words fall back to a character
                    // break (no text is dropped).
                    if let Some(pos) = chunk.rfind(' ') {
                        consumed = pos + 1;
                        chunk.truncate(pos);
                        chunk_w = display_width(&chunk);
                        word_break = true;
                    }
                    break;
                }
                chunk.push(c);
                chunk_w += cw;
            }
            if !chunk.is_empty() {
                let row = rows.last_mut().unwrap();
                if let Some(prev) = row.last_mut()
                    && prev.style == style
                {
                    prev.content.to_mut().push_str(&chunk);
                } else {
                    row.push(Span::styled(chunk, style));
                }
                *row_widths.last_mut().unwrap() += chunk_w;
            }
            rest = &rest[consumed..];
            if word_break {
                rows.push(Vec::new());
                row_widths.push(0);
            }
        }
    }
    rows
}

/// Pad styled spans to exactly `width` display cells, appending trailing
/// `bg`-styled spaces so a background box reads as one continuous band (pi's
/// Box paints the whole padded rect). Padding is measured with
/// [`display_width`] so double-width (CJK) content stays exact; content
/// already at or past `width` passes through untouched.
fn pad_line_to_width(spans: Vec<Span<'static>>, width: usize, bg: Style) -> Line<'static> {
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let pad = width.saturating_sub(used);
    let mut spans = spans;
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), bg));
    }
    Line::from(spans)
}

/// Pretty-print a tool arguments JSON string with two-space indentation;
/// falls back to the raw string when it is not valid JSON (or is empty).
fn pretty_args(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) => serde_json::to_string_pretty(&value)
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_else(|_| vec![raw.to_string()]),
        Err(_) => vec![raw.to_string()],
    }
}

/// Wrap one already-single-line string via `wrap_to_width`, re-joining to a
/// single string (used by the tool block's `  `-prefixed rows).
fn wrap_to_width_join(line: &str, width: usize) -> String {
    wrap_to_width(line, width).join(" ")
}

/// Restyle a markdown line to italic gray (thinking text).
fn italicize(line: Line<'static>, color: Color) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(|s| Span::styled(s.content, s.style.fg(color).add_modifier(Modifier::ITALIC)))
            .collect::<Vec<_>>(),
    )
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
            file: std::path::PathBuf::new(),
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

    /// The style of one buffer cell.
    fn cell_style(buffer: &Buffer, x: u16, y: u16) -> Style {
        buffer.cell((x, y)).unwrap().style()
    }

    /// Whether the whole row is filled with `bg` (`None` = some cell differs).
    fn row_bg_equals(buffer: &Buffer, y: u16, bg: Color) -> bool {
        cell_at_row(buffer, y).iter().all(|s| s.bg == Some(bg))
    }

    /// The styles of every cell in one row, skipping trailing fully-reset
    /// cells (the buffer's default blank tail).
    fn cell_at_row(buffer: &Buffer, y: u16) -> Vec<Style> {
        (0..buffer.area.width)
            .map(|x| cell_style(buffer, x, y))
            .collect()
    }

    /// Whether the row has `bg` at column `x` (used for scrollbar thumb
    /// cells, which sit in the rightmost column).
    fn cell_bg_at(buffer: &Buffer, x: u16, y: u16) -> Option<Color> {
        buffer.cell((x, y)).and_then(|c| c.style().bg)
    }

    /// The y of the first row containing `needle`.
    fn row_containing(buffer: &Buffer, needle: &str) -> Option<u16> {
        (0..buffer.area.height).find(|&y| line_at(buffer, y).contains(needle))
    }

    /// Whether any row has a `selectedBg` thumb cell in the rightmost column.
    fn has_thumb(buffer: &Buffer) -> bool {
        let x = buffer.area.width.saturating_sub(1);
        (0..buffer.area.height)
            .any(|y| cell_bg_at(buffer, x, y) == Some(BgToken::SelectedBg.color()))
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
        App::new("~/proj", "sess-1", "model-x", "9.9.9", vec![])
    }

    /// Render the transcript pane rows (content only, border excluded) of a
    /// seeded app at a fixed size, returning the visible content rows.
    fn transcript_window(app: &mut App, h: u16) -> Vec<String> {
        let buffer = render_buffer(app, 60, h);
        // The transcript pane is the top region: total height minus the input
        // box and footer. The pane is borderless now (pi-style full-bleed),
        // so rows map 1:1 from the top. The status indicator row exists only
        // while running (0 height when idle in these test apps).
        let pane_h = h - INPUT_HEIGHT - FOOTER_HEIGHT;
        (0..pane_h).map(|y| line_at(&buffer, y)).collect()
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
        // Consecutive reasoning deltas merge into one italic thinking block
        // (no `> ` glyph prefix any more).
        assert!(buffer_contains(&buffer, "The user said"));
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
    fn tool_start_pairs_with_result_into_one_block() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "read".to_string(),
            arguments: r#"{"path": "a.txt"}"#.to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "read".to_string(),
            ok: true,
            result: "ok".to_string(),
        })
        .unwrap();

        let buffer = render_buffer(&mut app, 60, 14);
        // Compact per-tool title + gray output, all inside one state-colored
        // block; the old `▶`/`✔` glyph lines are gone.
        assert!(buffer_contains(&buffer, "read a.txt"));
        assert!(buffer_contains(&buffer, "ok"));
        assert!(!buffer_contains(&buffer, "▶"));
        assert!(!buffer_contains(&buffer, "✔"));
    }

    #[test]
    fn failed_tool_result_sets_error_block() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "write".to_string(),
            arguments: "x".to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "write".to_string(),
            ok: false,
            result: "denied".to_string(),
        })
        .unwrap();

        // The failed result lands in the same block (no separate `✖` line).
        let buffer = render_buffer(&mut app, 60, 14);
        assert!(buffer_contains(&buffer, "denied"));
        assert!(!buffer_contains(&buffer, "✖"));
        assert!(!buffer_contains(&buffer, "⚠ stopped"));
    }

    #[test]
    fn completed_stop_renders_nothing_and_turn_marker_is_ignored() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Turn { turn: 1 }).unwrap();
        app.render(&DisplayItem::Stop(StopReason::Completed))
            .unwrap();
        app.render(&DisplayItem::Text("answer\n".to_string()))
            .unwrap();

        let buffer = render_buffer(&mut app, 60, 14);
        assert!(buffer_contains(&buffer, "answer"));
        assert!(!buffer_contains(&buffer, "── turn"));
        assert!(!buffer_contains(&buffer, "✓ done"));
    }

    #[test]
    fn cancelled_stop_renders_nothing_like_completed() {
        // Esc ends the turn silently: Stop(Cancelled) draws no marker and no
        // red error line — the partial transcript is the feedback (ticket 07).
        let mut app = seeded_app();
        app.render(&DisplayItem::Text("partial answer\n".to_string()))
            .unwrap();
        app.render(&DisplayItem::Stop(StopReason::Cancelled))
            .unwrap();
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(buffer_contains(&buffer, "partial answer"));
        assert!(!buffer_contains(&buffer, "⚠"));
        assert!(!buffer_contains(&buffer, "cancelled"));
        assert!(!buffer_contains(&buffer, "stopped"));
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
        // The prompt is echoed into the transcript as a boxed user block
        // (no `> ` notice line).
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(buffer_contains(&buffer, "explain tests"));
        assert!(!buffer_contains(&buffer, "> explain tests"));
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
        // The footer is reserved (ticket 03); no `ready` word any more.
        assert!(!buffer_contains(&small, "ready"));

        let wide = render_buffer(&mut app, 100, 16);
        assert!(buffer_contains(&wide, "resize me"));
        assert!(!buffer_contains(&wide, "ready"));
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
        assert!(buffer_contains(&buffer, "/skill:<name> runs one"));

        let mut app = App::new(
            "~/proj",
            "sess-1",
            "model-x",
            "9.9.9",
            vec![skill("grill", "stress-test a plan")],
        );
        type_text(&mut app, "/skills");
        assert_eq!(app.handle_key(key(KeyCode::Enter)), None);
        let buffer = render_buffer(&mut app, 100, 20);
        assert!(buffer_contains(&buffer, "skills:"));
        assert!(buffer_contains(&buffer, "/skill:grill"));
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
            "9.9.9",
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
            ..Default::default()
        }))
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(buffer_contains(
            &buffer,
            "tokens: 10 prompt (0 cached, 0%) + 5 completion = 15 total"
        ));
    }

    #[test]
    fn usage_renders_cached_count() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Usage(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: Some(slimcode_ai::wire::PromptTokensDetails {
                cached_tokens: 7,
                cache_creation_input_tokens: 3,
            }),
        }))
        .unwrap();
        let buffer = render_buffer(&mut app, 80, 12);
        assert!(buffer_contains(
            &buffer,
            "tokens: 10 prompt (7 cached, 70%) + 5 completion = 15 total"
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

    // --- `/` completion popup ---------------------------------------------

    #[test]
    fn typing_slash_opens_completion_with_all_candidates() {
        let mut app = seeded_app();
        type_text(&mut app, "/");
        let comp = app.completion.as_ref().expect("popup should open on /");
        assert!(comp.items.len() >= COMMANDS.len());
        assert_eq!(comp.selected, 0);
        assert!(comp.items.iter().any(|i| i.value == "/help"));
    }

    #[test]
    fn typing_filters_completion_to_matches() {
        let mut app = seeded_app();
        type_text(&mut app, "/sav");
        let comp = app.completion.as_ref().expect("popup open");
        let values: Vec<&str> = comp.items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, vec!["/save"]);
        assert_eq!(comp.selected, 0);
    }

    #[test]
    fn non_slash_input_does_not_open_completion() {
        let mut app = seeded_app();
        type_text(&mut app, "hello");
        assert!(app.completion.is_none());
    }

    #[test]
    fn space_closes_completion_for_arguments() {
        let mut app = seeded_app();
        type_text(&mut app, "/sav");
        assert!(app.completion.is_some());
        // Typing a space (entering the argument part) closes the popup.
        type_text(&mut app, " ");
        assert!(app.completion.is_none());
    }

    #[test]
    fn tab_commits_selection_with_trailing_space() {
        let mut app = seeded_app();
        type_text(&mut app, "/sav");
        let effect = app.handle_key(key(KeyCode::Tab));
        assert_eq!(effect, None);
        assert!(app.completion.is_none());
        // The selected candidate is committed with a trailing space, ready for
        // an argument; it is NOT submitted.
        assert_eq!(app.input_text(), "/save ");
    }

    #[test]
    fn tab_commits_alias_spelling() {
        let mut app = seeded_app();
        type_text(&mut app, "/res");
        app.handle_key(key(KeyCode::Tab));
        // `/res` fuzzy-matches the `/resume` alias; committing that spelling
        // still resolves to `/load` on submit.
        assert_eq!(app.input_text(), "/resume ");
    }

    #[test]
    fn enter_commits_selection_and_executes() {
        let mut app = seeded_app();
        type_text(&mut app, "/sav");
        // The popup's best match for `/sav` is `/save`; Enter expands and runs
        // it (Q4: execute the selection, not the literal partial text).
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, Some(Effect::SaveSession));
        assert!(app.completion.is_none());
        assert!(app.input_text().is_empty());
    }

    #[test]
    fn enter_runs_selected_not_typed_when_multiple_candidates() {
        let mut app = seeded_app();
        // `/s` matches several commands; the best match (first) is `/sessions`
        // (registry order, all score equally at a leading-s boundary hit).
        type_text(&mut app, "/s");
        let comp = app.completion.as_ref().unwrap();
        assert_eq!(comp.items[0].value, "/sessions");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(effect, Some(Effect::ListSessions));
    }

    #[test]
    fn enter_with_no_popup_submits_plain_input() {
        let mut app = seeded_app();
        type_text(&mut app, "plain prompt");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::SubmitPrompt("plain prompt".to_string()))
        );
    }

    #[test]
    fn up_down_navigates_completion_with_wrap() {
        let mut app = seeded_app();
        type_text(&mut app, "/");
        let len = app.completion.as_ref().unwrap().items.len();
        // Down moves forward.
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.completion.as_ref().unwrap().selected, 1);
        // Up wraps from the top to the last.
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.completion.as_ref().unwrap().selected, len - 1);
    }

    #[test]
    fn selection_survives_recompute_when_still_a_candidate() {
        let mut app = seeded_app();
        // `/s`: candidates include /sessions (first) and /save. Move down to
        // /save, then narrow to `/sa` where /save is still present: the
        // selection sticks to /save.
        type_text(&mut app, "/s");
        let before: Vec<&str> = app
            .completion
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|i| i.value.as_str())
            .collect();
        assert!(before.contains(&"/save"), "{before:?}");
        let save_idx = app
            .completion
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|i| i.value == "/save")
            .unwrap();
        for _ in 0..save_idx {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(app.completion.as_ref().unwrap().selected, save_idx);
        assert_eq!(
            app.completion.as_ref().unwrap().items[save_idx].value,
            "/save"
        );
        // Narrowing to `/sa` keeps /save selected (still the chosen value).
        type_text(&mut app, "a");
        let comp = app.completion.as_ref().unwrap();
        assert_eq!(comp.items[comp.selected].value, "/save");
    }

    #[test]
    fn esc_closes_completion_and_keeps_text() {
        let mut app = seeded_app();
        type_text(&mut app, "/sav");
        assert!(app.completion.is_some());
        app.handle_key(key(KeyCode::Esc));
        assert!(app.completion.is_none());
        assert_eq!(app.input_text(), "/sav");
    }

    #[test]
    fn backspace_into_command_reopens_completion() {
        let mut app = seeded_app();
        // Commit /save with Tab (trailing space, popup closed), then delete
        // the space: the popup reopens with /save still the best match.
        type_text(&mut app, "/sav");
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.input_text(), "/save ");
        assert!(app.completion.is_none());
        app.handle_key(key(KeyCode::Backspace));
        assert!(app.completion.is_some());
        let comp = app.completion.as_ref().unwrap();
        assert_eq!(comp.items[comp.selected].value, "/save");
    }

    #[test]
    fn page_keys_scroll_completion_not_transcript() {
        let mut app = seeded_app();
        // Seed a transcript taller than the pane so scrolling is observable.
        for i in 0..12 {
            app.render(&DisplayItem::Text(format!("line {i}\n")))
                .unwrap();
        }
        type_text(&mut app, "/");
        let scroll_before = app.scroll;
        app.handle_key(key(KeyCode::PageDown));
        // The transcript scroll is untouched; the popup selection advanced.
        assert_eq!(app.scroll, scroll_before);
        let comp = app.completion.as_ref().unwrap();
        assert!(comp.selected >= 10, "selected: {}", comp.selected);
    }

    #[test]
    fn completion_popup_renders_above_input_with_selection() {
        let mut app = seeded_app();
        type_text(&mut app, "/sav");
        let buffer = render_buffer(&mut app, 60, 16);
        // The candidate is drawn with its name and description, selected.
        assert!(buffer_contains(&buffer, "/save"));
        assert!(buffer_contains(&buffer, "save the current session"));
        // No bordered box and no ` completion ` title any more (ticket 02).
        assert!(!buffer_contains(&buffer, "completion"));
        let h: u16 = 16;
        let input_top = h - FOOTER_HEIGHT - INPUT_HEIGHT;
        // The popup rows sit ABOVE the input box's top border row.
        let popup_y = row_containing(&buffer, "→ /save").unwrap();
        assert!(
            popup_y < input_top,
            "popup above input: {popup_y} vs {input_top}"
        );
    }

    #[test]
    fn completion_popup_does_not_shift_the_input_box() {
        let mut app = seeded_app();
        let h: u16 = 16;
        let input_top = h - FOOTER_HEIGHT - INPUT_HEIGHT;
        // Closed: the input top border is at its anchored row.
        let closed = render_buffer(&mut app, 60, h);
        assert!(line_at(&closed, input_top).trim_matches('─').is_empty());
        // Open: the popup eats into the transcript; the input top border row
        // stays exactly where it was (input position stability).
        type_text(&mut app, "/sav");
        let open = render_buffer(&mut app, 60, h);
        assert_eq!(line_at(&open, input_top), line_at(&closed, input_top));
        let closed_top = line_at(&closed, input_top);
        assert!(closed_top.trim_matches('─').is_empty(), "{closed_top:?}");
    }

    #[test]
    fn completion_popup_has_top_separator_line() {
        let mut app = seeded_app();
        type_text(&mut app, "/sav");
        let buffer = render_buffer(&mut app, 60, 16);
        // The row directly above the first candidate is a full-width `─`
        // separator line, giving the popup a top border and a clear visual
        // gap from the transcript (ticket 05).
        let popup_y = row_containing(&buffer, "→ /save").unwrap();
        let sep_y = popup_y - 1;
        let sep = line_at(&buffer, sep_y);
        assert!(sep.starts_with('─'), "separator row: {sep:?}");
        assert_eq!(sep.trim_end_matches('─').len(), 0, "full-width: {sep:?}");
        // The separator uses the semantic border color, matching the editor.
        assert_eq!(
            cell_style(&buffer, 0, sep_y).fg,
            Some(Token::Border.color())
        );
    }

    #[test]
    fn completion_includes_installed_skills() {
        let mut app = App::new(
            "~/proj",
            "sess-1",
            "model-x",
            "9.9.9",
            vec![skill("grill", "stress-test a plan")],
        );
        type_text(&mut app, "/gr");
        let comp = app.completion.as_ref().expect("popup open");
        let values: Vec<&str> = comp.items.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"/skill:grill"), "{values:?}");
    }

    #[test]
    fn runtime_skill_install_refreshes_completion_and_dispatch() {
        let mut app = seeded_app();
        // No skills installed yet: `/gr` matches no command and no skill.
        type_text(&mut app, "/gr");
        assert!(app.completion.is_none());

        // A runtime `/install-skill` re-reads the store and pushes the fresh
        // snapshot into the app (the terminal loop wires this up).
        app.set_skills(vec![skill("grill", "stress-test a plan")]);

        // The next keystroke recomputes the popup: the new skill is
        // predictable without a restart.
        type_text(&mut app, "i");
        let comp = app.completion.as_ref().expect("popup open");
        let values: Vec<&str> = comp.items.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"/skill:grill"), "{values:?}");

        // And the freshly installed skill dispatches as a skill trigger.
        type_text(&mut app, "ll");
        let effect = app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            effect,
            Some(Effect::TriggerSkill {
                name: "grill".to_string(),
                arg: None
            })
        );
    }

    #[test]
    fn ctrl_c_quits_with_completion_open() {
        let mut app = seeded_app();
        type_text(&mut app, "/sav");
        assert_eq!(app.handle_key(ctrl_key('c')), Some(Effect::Quit));
    }

    // --- pi display alignment features (ticket 02) -------------------------

    #[test]
    fn header_renders_at_startup_and_new_clears_it() {
        let mut app = seeded_app();
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(buffer_contains(&buffer, "slimcode"));
        assert!(buffer_contains(&buffer, "v9.9.9"));
        assert!(buffer_contains(&buffer, "/help for commands"));
        // Brand line: bold accent `slimcode`.
        let y = row_containing(&buffer, "slimcode").unwrap();
        assert_eq!(cell_style(&buffer, 0, y).fg, Some(Token::Accent.color()));
        assert!(
            cell_style(&buffer, 0, y)
                .add_modifier
                .contains(Modifier::BOLD)
        );

        // `/new` clears the header together with the transcript.
        app.clear_for_new_session("sess-2");
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(!buffer_contains(&buffer, "slimcode"));
        assert!(buffer_contains(&buffer, "new session: sess-2"));
    }

    #[test]
    fn user_prompt_renders_as_boxed_message_with_bg() {
        let mut app = seeded_app();
        type_text(&mut app, "explain tests");
        app.handle_key(key(KeyCode::Enter));
        let buffer = render_buffer(&mut app, 60, 14);
        assert!(buffer_contains(&buffer, "explain tests"));
        // The prompt rows are full-width `userMessageBg`; no `> ` notice.
        let y = row_containing(&buffer, "explain tests").unwrap();
        assert!(row_bg_equals(&buffer, y, BgToken::UserMessageBg.color()));
        // Padding rows above and below are also bg-filled.
        assert_eq!(
            cell_bg_at(&buffer, 0, y.saturating_sub(1)),
            Some(BgToken::UserMessageBg.color())
        );
        assert_eq!(
            cell_bg_at(&buffer, 0, y.saturating_add(1)),
            Some(BgToken::UserMessageBg.color())
        );
        assert!(!buffer_contains(&buffer, "> explain"));
    }

    #[test]
    fn thinking_renders_italic_gray() {
        let mut app = seeded_app();
        app.render(&DisplayItem::Reasoning("let me think".to_string()))
            .unwrap();
        let buffer = render_buffer(&mut app, 60, 12);
        assert!(buffer_contains(&buffer, "let me think"));
        let y = row_containing(&buffer, "let me think").unwrap();
        let x = line_at(&buffer, y).find("let me think").unwrap() as u16;
        let style = cell_style(&buffer, x, y);
        assert_eq!(style.fg, Some(Token::ThinkingText.color()));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn tool_block_pending_then_success_backgrounds() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "read".to_string(),
            arguments: r#"{"path": "a.txt"}"#.to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 14);
        // Pending: the title row sits on `toolPendingBg`.
        let y = row_containing(&buffer, "read a.txt").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, 1, y),
            Some(BgToken::ToolPendingBg.color())
        );

        app.render(&DisplayItem::ToolResult {
            name: "read".to_string(),
            ok: true,
            result: "ok".to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 14);
        let y = row_containing(&buffer, "read a.txt").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, 1, y),
            Some(BgToken::ToolSuccessBg.color())
        );
    }

    /// Render one completed tool block (15-line output so the expand hint row
    /// exists) and return its frame buffer at 60x18.
    fn completed_tool_block_buffer(ok: bool) -> Buffer {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "read".to_string(),
            arguments: r#"{"path": "a.txt"}"#.to_string(),
        })
        .unwrap();
        let output: String = (0..15).map(|i| format!("out line {i}\n")).collect();
        app.render(&DisplayItem::ToolResult {
            name: "read".to_string(),
            ok,
            result: output,
        })
        .unwrap();
        render_buffer(&mut app, 60, 30)
    }

    /// Every row kind of a tool block — title, an output row, and the expand
    /// hint row — must carry the state background through the pane's rightmost
    /// column (ticket 05: pi Box paints the whole padded rect, not just the
    /// cells under the glyphs).
    #[test]
    fn tool_block_background_reaches_rightmost_column_for_every_row_kind() {
        let w = 60u16;
        let rightmost = w - 1;

        // Pending: the block is open and only the title row exists yet.
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "read".to_string(),
            arguments: r#"{"path": "a.txt"}"#.to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, w, 18);
        let y = row_containing(&buffer, "read").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, rightmost, y),
            Some(BgToken::ToolPendingBg.color()),
            "pending title row must reach the rightmost column"
        );
        assert!(
            row_bg_equals(&buffer, y, BgToken::ToolPendingBg.color()),
            "pending title row is a solid full-width band"
        );

        // Success: title, output, and hint rows all span the full width.
        let buffer = completed_tool_block_buffer(true);
        let expected = BgToken::ToolSuccessBg.color();
        let y = row_containing(&buffer, "read").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, rightmost, y),
            Some(expected),
            "success title row reaches the rightmost column"
        );
        let y = row_containing(&buffer, "out line 0").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, rightmost, y),
            Some(expected),
            "success output row reaches the rightmost column"
        );
        assert!(
            row_bg_equals(&buffer, y, expected),
            "success output row is a solid full-width band"
        );
        let y = row_containing(&buffer, "more lines").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, rightmost, y),
            Some(expected),
            "success expand-hint row reaches the rightmost column"
        );

        // Error: same three row kinds on `toolErrorBg`.
        let buffer = completed_tool_block_buffer(false);
        let expected = BgToken::ToolErrorBg.color();
        let y = row_containing(&buffer, "read").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, rightmost, y),
            Some(expected),
            "error title row reaches the rightmost column"
        );
        let y = row_containing(&buffer, "out line 0").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, rightmost, y),
            Some(expected),
            "error output row reaches the rightmost column"
        );
        let y = row_containing(&buffer, "more lines").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, rightmost, y),
            Some(expected),
            "error expand-hint row reaches the rightmost column"
        );
    }

    /// A tool output row ending in wide CJK glyphs still pads exactly to the
    /// pane width: padding is measured in display columns, never in chars.
    #[test]
    fn tool_block_padding_is_display_width_exact_for_wide_chars() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "read".to_string(),
            arguments: r#"{"path": "a.txt"}"#.to_string(),
        })
        .unwrap();
        // 30 CJK chars = 60 display columns — wider than the 58-col content
        // area, so wrapping plus width-exact padding must still end at column
        // 59 with the state background (nothing truncated, nothing short).
        let output: String = (0..12).map(|i| format!("\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}\u{597d}line {i}\n")).collect();
        app.render(&DisplayItem::ToolResult {
            name: "read".to_string(),
            ok: true,
            result: output,
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 18);
        let bg = BgToken::ToolSuccessBg.color();
        // Every row between the first output row and the bottom spacer must be
        // full-width (a short visual band would end before the last column).
        let first = row_containing(&buffer, "好").unwrap();
        let hint_y = row_containing(&buffer, "more lines").unwrap();
        for y in first..=hint_y {
            assert_eq!(
                cell_bg_at(&buffer, 59, y),
                Some(bg),
                "row {y} must carry the state bg to the rightmost column"
            );
        }
    }

    #[test]
    fn tool_block_error_background_and_compact_title() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "edit".to_string(),
            arguments: r#"{"path":"a.txt","old":"x"}"#.to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "edit".to_string(),
            ok: false,
            result: "no match".to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 16);
        let y = row_containing(&buffer, "edit a.txt").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, 1, y),
            Some(BgToken::ToolErrorBg.color())
        );
        // Built-in tools show a compact `edit <path>` title — no raw JSON args
        // section (ticket 06).
        assert!(!buffer_contains(&buffer, "\"path\":"));
        assert!(buffer_contains(&buffer, "no match"));
    }

    #[test]
    fn builtin_tool_blocks_never_show_json_args_section() {
        // read with a 1-indexed offset/limit range: the compact title carries
        // the range; the raw JSON never appears.
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "read".to_string(),
            arguments: r#"{"path":"a.txt","offset":2,"limit":3}"#.to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "read".to_string(),
            ok: true,
            result: "line2\nline3\nline4".to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 16);
        assert!(buffer_contains(&buffer, "read a.txt:2-4"));
        assert!(!buffer_contains(&buffer, "\"path\":"));
        assert!(!buffer_contains(&buffer, "\"offset\":"));

        // bash renders the whole call line as `$ command` (bold), and grep
        // its `/pattern/ in <scope>` shape — again without a JSON dump.
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "bash".to_string(),
            arguments: r#"{"command":"ls -la"}"#.to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "bash".to_string(),
            ok: true,
            result: "total 8".to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolStart {
            name: "grep".to_string(),
            arguments: r#"{"pattern":"TODO","path":"src"}"#.to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "grep".to_string(),
            ok: true,
            result: "src/main.rs:1: TODO".to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 22);
        assert!(buffer_contains(&buffer, "$ ls -la"));
        assert!(buffer_contains(&buffer, "grep /TODO/ in src"));
        assert!(!buffer_contains(&buffer, "\"command\":"));
        assert!(!buffer_contains(&buffer, "\"pattern\":"));
    }

    #[test]
    fn unknown_tool_block_keeps_pretty_json_args_fallback() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "fetch_web".to_string(),
            arguments: r#"{"url":"https://x","depth":2}"#.to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "fetch_web".to_string(),
            ok: true,
            result: "<html>".to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 16);
        // Unknown tools have no compact shape: pi's fallback (bold name + the
        // args as pretty JSON) is what the user sees.
        assert!(buffer_contains(&buffer, "fetch_web"));
        assert!(buffer_contains(&buffer, "\"url\": \"https://x\""));
        assert!(buffer_contains(&buffer, "<html>"));
    }

    #[test]
    fn tool_call_title_wraps_at_pane_width_without_losing_text() {
        let mut app = seeded_app();
        let long_command = format!(
            "echo {}",
            (0..40)
                .map(|i| format!("word{i:02}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        app.render(&DisplayItem::ToolStart {
            name: "bash".to_string(),
            arguments: serde_json::json!({"command": long_command}).to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "bash".to_string(),
            ok: true,
            result: "done".to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 16);
        // The wrapped title keeps every word (the last one only appears on a
        // wrapped row) and the block stays a padded full-width band.
        assert!(buffer_contains(&buffer, "$ echo"));
        assert!(buffer_contains(&buffer, "word39"));
        let y = row_containing(&buffer, "done").unwrap();
        assert_eq!(
            cell_bg_at(&buffer, 59, y),
            Some(BgToken::ToolSuccessBg.color())
        );
    }

    #[test]
    fn tool_output_collapses_to_ten_lines_and_ctrl_o_expands() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "grep".to_string(),
            arguments: "{\"pattern\":\"x\"}".to_string(),
        })
        .unwrap();
        let output: String = (0..15).map(|i| format!("line{i}\n")).collect();
        app.render(&DisplayItem::ToolResult {
            name: "grep".to_string(),
            ok: true,
            result: output.clone(),
        })
        .unwrap();

        let buffer = render_buffer(&mut app, 60, 30);
        let collapsed = line_at(&buffer, row_containing(&buffer, "line0").unwrap());
        // Collapsed: first 10 lines + the expand hint; `line10` hidden.
        assert!(buffer_contains(&buffer, "line0"));
        assert!(buffer_contains(&buffer, "line9"));
        assert!(!buffer_contains(&buffer, "line10"));
        assert!(buffer_contains(&buffer, "5 more lines, Ctrl+O to expand"));
        let _ = collapsed;

        // Ctrl+O expands globally: all 15 lines, hint gone.
        app.handle_key(ctrl_key('o'));
        let buffer = render_buffer(&mut app, 60, 30);
        assert!(buffer_contains(&buffer, "line10"));
        assert!(buffer_contains(&buffer, "line14"));
        assert!(!buffer_contains(&buffer, "more lines"));

        // Ctrl+O again collapses.
        app.handle_key(ctrl_key('o'));
        let buffer = render_buffer(&mut app, 60, 30);
        assert!(!buffer_contains(&buffer, "line10"));
        assert!(buffer_contains(&buffer, "5 more lines, Ctrl+O to expand"));
        assert!(!app.tool_output_expanded);
        assert!(!app.status.running); // untouched by Ctrl+O
    }

    #[test]
    fn tool_output_under_ten_lines_has_no_hint() {
        let mut app = seeded_app();
        app.render(&DisplayItem::ToolStart {
            name: "ls".to_string(),
            arguments: "{\"path\":\".\"}".to_string(),
        })
        .unwrap();
        app.render(&DisplayItem::ToolResult {
            name: "ls".to_string(),
            ok: true,
            result: "a\nb".to_string(),
        })
        .unwrap();
        let buffer = render_buffer(&mut app, 60, 14);
        assert!(buffer_contains(&buffer, "ls ."));
        assert!(buffer_contains(&buffer, "a"));
        assert!(buffer_contains(&buffer, "b"));
        assert!(!buffer_contains(&buffer, "more lines"));
    }

    #[test]
    fn scrollbar_appears_on_scroll_and_fades_on_ticks() {
        let mut app = seeded_app();
        for i in 0..12 {
            app.render(&DisplayItem::Text(format!("line {i}\n")))
                .unwrap();
        }
        // At the bottom (follow), no thumb.
        let buffer = render_buffer(&mut app, 60, 8);
        assert!(!has_thumb(&buffer));

        app.handle_key(key(KeyCode::PageUp));
        let buffer = render_buffer(&mut app, 60, 8);
        assert!(has_thumb(&buffer), "thumb should appear after scrolling");
        let thumb_fg = cell_bg_at(&buffer, 59, 0).is_some();
        let _ = thumb_fg;

        // Auto fade: after SCROLLBAR_FADE_TICKS frames the thumb is gone.
        for _ in 0..SCROLLBAR_FADE_TICKS {
            app.tick();
        }
        let buffer = render_buffer(&mut app, 60, 8);
        assert!(!has_thumb(&buffer), "thumb should fade after ticks");
    }

    #[test]
    fn editor_border_color_reflects_running_state() {
        let mut app = seeded_app();
        // Input top-border row: y = h - FOOTER_HEIGHT - INPUT_HEIGHT.
        let h: u16 = 12;
        let border_y = h - FOOTER_HEIGHT - INPUT_HEIGHT;
        let buffer = render_buffer(&mut app, 60, h);
        assert_eq!(
            cell_style(&buffer, 5, border_y).fg,
            Some(Token::Border.color())
        );

        app.set_running(true);
        let buffer = render_buffer(&mut app, 60, h);
        assert_eq!(
            cell_style(&buffer, 5, border_y).fg,
            Some(Token::BorderAccent.color())
        );
        // The running border color also carries the embedded status text on
        // the same (top border) row (ticket 01).
        assert!(line_at(&buffer, border_y).contains("Working..."));
    }

    #[test]
    fn completion_popup_uses_select_list_tokens() {
        let mut app = seeded_app();
        type_text(&mut app, "/save");
        let buffer = render_buffer(&mut app, 80, 16);
        // Selected row: `→ ` cursor + accent name (no bold, no bg
        // inversion — pi select-list `selectedText`).
        assert!(buffer_contains(&buffer, "→ /save"));
        let y = row_containing(&buffer, "→ /save").unwrap();
        let x = line_at(&buffer, y).find('/').unwrap() as u16;
        assert_eq!(cell_style(&buffer, x, y).fg, Some(Token::Accent.color()));
        // Description is `muted` (pi select-list `description` token).
        let desc_y = row_containing(&buffer, "save the current session").unwrap();
        let desc_x = line_at(&buffer, desc_y)
            .find("save the current session")
            .unwrap() as u16;
        assert_eq!(
            cell_style(&buffer, desc_x, desc_y).fg,
            Some(Token::Muted.color())
        );
    }

    #[test]
    fn completion_popup_shows_scroll_info_when_overflowing() {
        let mut app = seeded_app();
        type_text(&mut app, "/");
        let buffer = render_buffer(&mut app, 80, 20);
        // Many candidates: the muted `(i/n)` scroll info renders.
        assert!(buffer_contains(&buffer, "(1/"));
        let y = row_containing(&buffer, "(1/").unwrap();
        assert_eq!(cell_style(&buffer, 2, y).fg, Some(Token::Muted.color()));
    }

    #[test]
    fn notice_renders_dim() {
        let mut app = seeded_app();
        app.push_notice("hello notice");
        let buffer = render_buffer(&mut app, 60, 12);
        let y = row_containing(&buffer, "hello notice").unwrap();
        assert_eq!(cell_style(&buffer, 0, y).fg, Some(Token::Dim.color()));
    }

    #[test]
    fn error_renders_red() {
        let mut app = seeded_app();
        app.push_error("boom");
        let buffer = render_buffer(&mut app, 60, 12);
        let y = row_containing(&buffer, "boom").unwrap();
        assert_eq!(cell_style(&buffer, 0, y).fg, Some(Token::Error.color()));
    }

    // --- footer + status indicator (ticket 03) -----------------------------

    #[test]
    fn footer_renders_two_dim_lines() {
        let mut app = seeded_app();
        app.set_branch(Some("main".to_string()));
        app.set_usage(crate::footer::FooterUsage {
            input: 1500,
            output: 500,
            cache_read: 0,
            cache_write: 0,
        });
        let h: u16 = 12;
        let buffer = render_buffer(&mut app, 60, h);
        // Footer occupies the bottom two rows.
        let line1 = line_at(&buffer, h - 2);
        assert!(line1.contains("~/proj (main) • sess-1"), "{line1:?}");
        assert_eq!(cell_style(&buffer, 0, h - 2).fg, Some(Token::Dim.color()));
        let line2 = line_at(&buffer, h - 1);
        assert!(line2.contains("↑1.5k ↓500"), "{line2:?}");
        assert!(line2.ends_with("model-x"), "{line2:?}");
        assert_eq!(cell_style(&buffer, 0, h - 1).fg, Some(Token::Dim.color()));
    }

    #[test]
    fn footer_omits_branch_and_usage_when_absent() {
        let mut app = seeded_app();
        let h: u16 = 12;
        let buffer = render_buffer(&mut app, 60, h);
        let line1 = line_at(&buffer, h - 2);
        assert!(line1.contains("~/proj • sess-1"), "{line1:?}");
        assert!(!line1.contains("("), "branch should be omitted");
        // No usage yet: stats line has no stats, just the model right-aligned.
        let line2 = line_at(&buffer, h - 1);
        assert!(line2.trim_end().ends_with("model-x"), "{line2:?}");
    }

    #[test]
    fn runner_status_embedded_in_input_top_border_and_animates() {
        let mut app = seeded_app();
        let h: u16 = 12;
        let border_y = h - FOOTER_HEIGHT - INPUT_HEIGHT;
        // Idle: the input top border is a plain full-width `─` line, no status.
        let idle = render_buffer(&mut app, 60, h);
        let line = line_at(&idle, border_y);
        assert!(!line.contains("Working"), "{line:?}");
        assert_eq!(
            line.trim_end_matches('─').len(),
            0,
            "plain border: {line:?}"
        );

        // Running: the status sits on the input top border, left-aligned
        // (`── ⠋ Working... ────`), like pi's embedWorkingStatus.
        app.set_running(true);
        let buffer = render_buffer(&mut app, 60, h);
        let line = line_at(&buffer, border_y);
        assert!(line.starts_with("── "), "status prefix: {line:?}");
        assert!(line.contains("Working..."), "{line:?}");
        // The whole status+border row is the running border color (pi embeds
        // the status in the border and colors them together).
        let spinner_x = 3; // after `── `
        assert_eq!(
            cell_style(&buffer, spinner_x, border_y).fg,
            Some(Token::BorderAccent.color())
        );
        assert_eq!(
            cell_style(&buffer, spinner_x + 2, border_y).fg,
            Some(Token::BorderAccent.color())
        );

        // tick() advances the braille frame (each of the 10 frames differs).
        let frame1 = buffer
            .cell((spinner_x, border_y))
            .unwrap()
            .symbol()
            .to_string();
        app.tick();
        let buffer2 = render_buffer(&mut app, 60, h);
        let frame2 = buffer2
            .cell((spinner_x, border_y))
            .unwrap()
            .symbol()
            .to_string();
        assert_ne!(frame1, frame2);

        // Idle again: the border row is a plain `─` line again.
        app.set_running(false);
        let idle2 = render_buffer(&mut app, 60, h);
        let line2 = line_at(&idle2, border_y);
        assert!(!line2.contains("Working"), "{line2:?}");
        assert_eq!(
            line2.trim_end_matches('─').len(),
            0,
            "plain border: {line2:?}"
        );
    }

    #[test]
    fn running_keys_ignore_all_except_ctrl_c_d() {
        let mut app = seeded_app();
        // Ordinary keys are ignored while a turn runs.
        assert_eq!(app.handle_key_running(key(KeyCode::Char('a'))), None);
        assert_eq!(app.handle_key_running(key(KeyCode::Enter)), None);
        assert_eq!(app.handle_key_running(ctrl_key('o')), None);
        // Ctrl+C / Ctrl+D arm quit-after-turn.
        assert_eq!(
            app.handle_key_running(ctrl_key('c')),
            Some(Effect::QuitAfterTurn)
        );
        assert_eq!(
            app.handle_key_running(ctrl_key('d')),
            Some(Effect::QuitAfterTurn)
        );
        // Idle state untouched by running keys.
        assert!(!app.status.running);
    }

    #[test]
    fn bare_escape_cancels_the_running_turn() {
        let mut app = seeded_app();
        // Esc while a turn runs → CancelRunning (ticket 07); Ctrl+C/Ctrl+D
        // semantics are unchanged (quit after the turn).
        assert_eq!(
            app.handle_key_running(key(KeyCode::Esc)),
            Some(Effect::CancelRunning)
        );
        // Modified Esc (shift/ctrl/alt) is not a cancel — ticket 07 says bare
        // Esc.
        assert_eq!(
            app.handle_key_running(KeyEvent::new(KeyCode::Esc, KeyModifiers::SHIFT)),
            None,
            "shift+Esc is not a cancel"
        );
        assert_eq!(
            app.handle_key_running(KeyEvent::new(KeyCode::Esc, KeyModifiers::CONTROL)),
            None,
            "ctrl+Esc is not a cancel"
        );
        assert_eq!(
            app.handle_key_running(KeyEvent::new(KeyCode::Esc, KeyModifiers::ALT)),
            None,
            "alt+Esc is not a cancel"
        );
        // A cancel never touches the quit-after-turn flag or running state.
        assert!(!app.status.running);
    }
}
