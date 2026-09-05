//! Shared frontend setup (ADR-0004): build the provider and the tool set for
//! a working directory. Both the one-shot CLI and the TUI construct their
//! runtime from this helper, so the provider/tools wiring cannot drift between
//! frontends.

use std::path::Path;

use slimcode_agent::agent::{CancelToken, Tool};
use slimcode_ai::{BailianConfig, BailianProvider};

use crate::tools;

/// Build the provider from a resolved config and the seven-tool set bound to
/// `cwd` (the plain, non-cancellable tools — used by the one-shot CLI).
/// Provider construction validates the config (API key, URL); callers
/// must run this before opening any full-screen UI so failures surface on the
/// normal terminal.
pub fn setup(cwd: &Path, config: BailianConfig) -> Result<(BailianProvider, Vec<Tool>), String> {
    let provider = BailianProvider::new(config)?;
    let tools = tools::build_tools(cwd);
    Ok((provider, tools))
}

/// Build the provider and the cancellable tool set (ticket 07) — used by the
/// TUI so a long-running `bash` child observes an Esc cancel. Identical to
/// [`setup`] otherwise.
pub fn setup_with_cancel(
    cwd: &Path,
    config: BailianConfig,
    cancel: &CancelToken,
) -> Result<(BailianProvider, Vec<Tool>), String> {
    let provider = BailianProvider::new(config)?;
    let tools = tools::build_tools_with_cancel(cwd, cancel);
    Ok((provider, tools))
}
