//! Shared frontend setup (ADR-0004): build the provider and the tool set for
//! a working directory. Both the one-shot CLI and the TUI construct their
//! runtime from this helper, so the provider/tools wiring cannot drift between
//! frontends.

use std::path::Path;

use slimcode_agent::agent::Tool;
use slimcode_ai::{BailianConfig, BailianProvider};

use crate::tools;

/// Build the provider from a resolved config and the seven-tool set bound to
/// `cwd`. Provider construction validates the config (API key, URL); callers
/// must run this before opening any full-screen UI so failures surface on the
/// normal terminal.
pub fn setup(cwd: &Path, config: BailianConfig) -> Result<(BailianProvider, Vec<Tool>), String> {
    let provider = BailianProvider::new(config)?;
    let tools = tools::build_tools(cwd);
    Ok((provider, tools))
}
