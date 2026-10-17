import { CHIP_TONE } from "../chip";
import type { ChipCategory } from "../chip";

// The shared chip badge: an inline rounded, monospace badge wrapping a leading
// `/` token. One colour source (chip.ts CHIP_TONE) drives the transcript user
// block, the floating input chip, and the inline input highlight.

export function ChipBadge({
  category,
  token,
  className = "",
}: {
  category: ChipCategory;
  token: string;
  className?: string;
}) {
  return (
    <span
      className={`inline-flex items-center rounded px-1.5 py-0.5 font-mono text-xs font-semibold ring-1 ring-inset ${CHIP_TONE[category]} ${className}`}
    >
      {token}
    </span>
  );
}
