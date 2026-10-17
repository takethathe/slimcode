// Shared token-count formatting for the transcript and the top bar: show
// thousands as `Nk` / `N.Nk` (mirrors the CLI footer's compact numbers).

export function formatTokens(n: number): string {
  if (n >= 1000) {
    const k = n / 1000;
    return `${k % 1 === 0 ? k.toFixed(0) : k.toFixed(1)}k`;
  }
  return String(n);
}

// Collapse text onto one line and clip it to `max` chars with a trailing
// ellipsis, for block titles that preview longer content (mirrors the Rust
// side's 48-char truncation).
export function truncateLine(text: string, max: number): string {
  const oneLine = text.replace(/\s+/g, " ").trim();
  if (oneLine.length <= max) return oneLine;
  return `${oneLine.slice(0, max)}…`;
}
