//! The public **Engine** construction seam (ADR-0031 D1/D5, engine ticket 05):
//! one build picks a preset (the code default hook bundle, or a host-supplied
//! [`PhaseHooks`] bundle) and injects the collaborators the loop runs over —
//! its operands ([`Provider`] / [`Tool`]), a [`Tracer`], and run configuration.
//!
//! Everything the loop does is reachable through this surface; the kernel
//! itself ([`slimcode_core::agent::AgentRunner`]) stays the single
//! non-replaceable core and is *not* part of the public API a preset host
//! touches. A "code agent" and a "bot agent" are the same Engine with a
//! different [`Preset`] bundle — the code preset is exactly what
//! [`AgentSession`](slimcode_sdk::AgentSession) assembles, and the SDK builds
//! its turns through this seam so its product behaviors are wired the same way
//! any external host would wire them (defaults enjoy no privileged internals).
//!
//! An Engine is single-use: it borrows its operands for one [`Engine::run_turn`]
//! (the host keeps ownership across turns, taking/putting its provider the same
//! way the SDK does). `Tracer: None` and an empty hook bundle leave the run
//! byte-identical to the plain [`crate::runner::run_turn`] — the equivalence
//! gate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use slimcode_core::agent::{
    AgentMessage, CancelToken, PhaseHooks, Provider, ProviderConfig, RunConfig, StopReason, Tool,
    Tracer,
};

use crate::context::Context;
use crate::preset;
use crate::render::Renderer;
use crate::runner::{self, MessageSink};
use crate::skills::Skill;

/// Which default-hook bundle an [`Engine`] runs (ADR-0031 D5).
pub enum Preset {
    /// slimcode's coding-agent preset: the code default bundle — today the
    /// `pre_tool` skill-read interception (ADR-0031 D2). `skills` / `cwd` are
    /// the installed skills and working directory the interception matches
    /// against; `history` is the message snapshot it dedups against (the
    /// turn's history at its start — the CLI's `history_for_skill_dedup`), so a
    /// second activation of a skill in one turn still reads as already-loaded.
    Code {
        skills: Vec<Skill>,
        cwd: PathBuf,
        history: Vec<AgentMessage>,
    },
    /// A host brings its own seven-phase bundle (a bot preset, or a custom
    /// embed): the engine runs exactly those hooks and no defaults of its own.
    Custom(PhaseHooks<'static>),
}

impl Default for Preset {
    /// No hooks — the bare loop (byte-identical to a run with an empty
    /// [`PhaseHooks`]).
    fn default() -> Self {
        Preset::Custom(PhaseHooks::default())
    }
}

/// A turn driven over borrowed operands. Built once per turn by
/// [`Engine::new`] plus the `with_*` setters (or [`Engine::code`] for the code
/// preset), then run once with [`Engine::run_turn`].
pub struct Engine<'a> {
    provider: &'a mut Box<dyn Provider + Send>,
    tools: &'a [Tool],
    provider_config: &'a ProviderConfig,
    run_config: RunConfig,
    tracer: Option<Arc<dyn Tracer>>,
    preset: Option<Preset>,
}

impl<'a> Engine<'a> {
    /// Start an Engine over the given operands with no hooks and no tracer (the
    /// bare loop). Layer on a [`Preset`] / [`Tracer`] / [`RunConfig`] with the
    /// `with_*` setters.
    pub fn new(
        provider: &'a mut Box<dyn Provider + Send>,
        tools: &'a [Tool],
        provider_config: &'a ProviderConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            provider_config,
            run_config: RunConfig::default(),
            tracer: None,
            preset: Some(Preset::default()),
        }
    }

    /// Assemble the **code preset** (ADR-0031 D5): the same operand set plus the
    /// skill-read interception bundle. This is the seam
    /// [`AgentSession`](slimcode_sdk::AgentSession) builds its turns on, so the
    /// code behaviors ride the public surface rather than a privileged path.
    pub fn code(
        provider: &'a mut Box<dyn Provider + Send>,
        tools: &'a [Tool],
        provider_config: &'a ProviderConfig,
        skills: &[Skill],
        cwd: &Path,
        history: Vec<AgentMessage>,
    ) -> Self {
        let mut engine = Self::new(provider, tools, provider_config);
        engine.preset = Some(Preset::Code {
            skills: skills.to_vec(),
            cwd: cwd.to_path_buf(),
            history,
        });
        engine
    }

    /// Set the run configuration (parallel vs serial tool dispatch). Defaults to
    /// [`RunConfig::default()`].
    pub fn with_run_config(mut self, run_config: RunConfig) -> Self {
        self.run_config = run_config;
        self
    }

    /// Choose the hook bundle to run ([`Preset::Code`] for the code defaults,
    /// [`Preset::Custom`] to bring your own).
    pub fn with_preset(mut self, preset: Preset) -> Self {
        self.preset = Some(preset);
        self
    }

    /// Inject the [`Tracer`] the kernel opens operand-call spans on (ADR-0031
    /// D4). Without it every span is a silent no-op.
    pub fn with_tracer(mut self, tracer: Arc<dyn Tracer>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Drive one agent turn over `context`, streaming to `renderer` and
    /// forwarding every history-entering message to `on_message`, exactly as
    /// [`crate::runner::run_turn`] does — through the assembled preset hooks and
    /// tracer. Consumes the Engine's [`Preset`] (single-use per turn).
    pub fn run_turn(
        &mut self,
        context: Context,
        cancel: &CancelToken,
        renderer: &mut dyn Renderer,
        on_message: MessageSink<'_>,
    ) -> Result<(Vec<AgentMessage>, StopReason), String> {
        let preset = self
            .preset
            .take()
            .expect("an Engine runs one turn per construction");
        let phases = match preset {
            Preset::Code {
                skills,
                cwd,
                history,
            } => preset::code_phase_hooks(&skills, &cwd, history),
            Preset::Custom(phases) => phases,
        };
        runner::run_turn_with_pipeline(
            self.provider,
            self.tools,
            context,
            &self.run_config,
            self.provider_config,
            cancel,
            renderer,
            on_message,
            phases,
            self.tracer.clone(),
        )
    }
}
