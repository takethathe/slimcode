//! The library's half of the runtime seam: the frame loop and the turn worker
//! (ADR-0013 D2).
//!
//! This is UI mechanics only. Setup, sessions, commands and every other
//! application decision arrive through [`UiHandler`], and the display state the
//! loop applies is [`RenderItem`]. The caller (the CLI) owns the process: raw
//! mode, the alternate screen and the exit code are all outside this module.

use std::io::Stdout;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{App, Effect};
use crate::handler::{ControlFlow, Prompt, UiHandler};
use crate::render::RenderItem;

/// One UI-loop frame: ~80ms, matching pi's loader interval so the status
/// spinner animates at the same rate and the input stays responsive.
const FRAME_MS: u64 = 80;

/// Run the frame loop until the user quits.
///
/// The caller builds the terminal and the [`App`]; `handler` answers the
/// reducer, and every `RenderItem` it emits is applied to the app. A turn runs
/// on a worker thread so the frame loop keeps polling input, draining the
/// channel and drawing while the provider call blocks.
pub fn run<H: UiHandler>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: App,
    handler: &mut H,
) -> Result<(), String> {
    let mut quit_after_turn = false;
    // Paint the first frame before blocking on input, so the startup header is
    // visible immediately.
    draw(terminal, &mut app)?;
    loop {
        let event = event::read().map_err(|e| e.to_string())?;
        match event {
            Event::Key(key) => {
                if let Some(effect) = app.handle_key(key) {
                    match handler.on_effect(effect, &mut |item| app.apply(item)) {
                        ControlFlow::Continue => {}
                        ControlFlow::Quit => return Ok(()),
                        ControlFlow::Submit(prompt) => {
                            drive_turn(terminal, &mut app, handler, prompt, &mut quit_after_turn)?;
                        }
                    }
                }
            }
            // Resize re-renders: ratatui's draw autoresizes, so the next frame
            // already uses the new size.
            Event::Resize(..) => {}
            _ => {}
        }
        // Ctrl+C/Ctrl+D while a turn ran arm quit-after-turn; once the turn is
        // over, leave the loop.
        if quit_after_turn {
            return Ok(());
        }
        draw(terminal, &mut app)?;
        app.tick();
    }
}

/// Run one turn on a worker thread and pump the frame loop until it ends.
///
/// The worker only exists so the spinner keeps animating and Esc/Ctrl+C keep
/// working during a blocking provider call: the frame loop stays on this
/// thread, draining the channel the worker's `emit` feeds. `handler.cancel()`
/// is called on this thread (hence `&self` on the trait), and the provider and
/// tools are the CLI's business either way.
fn drive_turn<H: UiHandler>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    handler: &H,
    prompt: Prompt,
    quit_after_turn: &mut bool,
) -> Result<(), String> {
    app.set_running(true);
    let (tx, rx) = mpsc::channel::<RenderItem>();
    let worker = thread::scope(|scope| {
        let handle = scope.spawn(move || {
            let mut emit = |item: RenderItem| {
                // A closed channel means the loop is already gone; the turn
                // still finishes so the session log stays consistent.
                let _ = tx.send(item);
            };
            handler.submit(prompt, &mut emit)
        });

        // A terminal/UI error is remembered but the loop still drains the
        // channel and joins, so the turn result is never lost. A join panic is
        // the same unrecoverable worker loss as before.
        let mut ui_error: Option<String> = None;
        let joined = loop {
            // Polling is also the frame timer (~80ms); on a failed poll once
            // an error is recorded we still sleep so the loop is not hot.
            match event::poll(Duration::from_millis(FRAME_MS)) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) if ui_error.is_none() => {
                        match app.handle_key_running(key) {
                            Some(Effect::QuitAfterTurn) => *quit_after_turn = true,
                            // Esc: abort the in-flight request / tool at the
                            // next runner boundary; the worker drains and joins
                            // as usual and the turn ends cancelled.
                            Some(Effect::CancelRunning) => handler.cancel(),
                            _ => {}
                        }
                    }
                    Ok(_) => {}
                    Err(e) if ui_error.is_none() => ui_error = Some(format!("event: {e}")),
                    Err(_) => {}
                },
                Ok(false) => {}
                Err(e) if ui_error.is_none() => ui_error = Some(format!("event poll: {e}")),
                Err(_) => {}
            }
            drain(&rx, app);
            if handle.is_finished() {
                drain(&rx, app);
                break handle
                    .join()
                    .map_err(|_| "turn worker panicked".to_string())?;
            }
            if ui_error.is_none() {
                draw(terminal, app)?;
            }
            app.tick();
        };
        Ok::<_, String>((joined, ui_error))
    })?;
    app.set_running(false);
    let (report, ui_error) = worker;
    if let Some(err) = ui_error {
        return Err(err);
    }
    // A failed turn renders as an error line; the handler has already closed
    // the session log on an assistant boundary.
    if let Err(e) = report {
        app.apply(RenderItem::Error(e));
    }
    Ok(())
}

/// Drain every item the worker has queued into the app, in order.
fn drain(rx: &mpsc::Receiver<RenderItem>, app: &mut App) {
    while let Ok(item) = rx.try_recv() {
        app.apply(item);
    }
}

/// Draw one frame.
fn draw(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<(), String> {
    terminal
        .draw(|frame| app.draw(frame, frame.area()))
        .map(|_| ())
        .map_err(|e| e.to_string())
}
