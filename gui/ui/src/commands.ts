// The slash Commands the GUI frontend intercepts (spec D3): each maps to a
// local GUI action and never reaches the host `prompt` path. This is the single
// source of truth shared by the InputBox submit dispatch and the HelpModal
// "runs locally" tag, so the two can never drift apart.

export const INTERCEPTED_COMMANDS = new Set([
  "/new",
  "/compact",
  "/usage",
  "/exit",
  "/help",
]);
