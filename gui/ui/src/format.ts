// Shared token-count formatting for the transcript and the top bar: show
// thousands as `Nk` / `N.Nk` (mirrors the CLI footer's compact numbers).

export function formatTokens(n: number): string {
  if (n >= 1000) {
    const k = n / 1000;
    return `${k % 1 === 0 ? k.toFixed(0) : k.toFixed(1)}k`;
  }
  return String(n);
}
