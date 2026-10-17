// The shared chip vocabulary for both sides of a `/` token: the input-side
// Command chip and the submitted-side Transcript chip colour a leading token by
// category — a Skill renders teal, a Command amber — from one source of truth
// so the two surfaces can never drift (GLOSSARY: Command chip / Transcript chip).

export type ChipCategory = "skill" | "command";

/** A chip as it appears in a transcript user block: the reconstructed token
 *  (slash included) plus its colour category. */
export type Chip = { category: ChipCategory; token: string };

/** Categorise a committed candidate spelling. Skills are advertised under the
 *  canonical `/skill:{name}` value (the completion pool's skill spelling), so a
 *  leading `/skill:` marks a Skill; every other recognised token is a Command. */
export function chipCategoryForCommitted(value: string): ChipCategory {
  return value.startsWith("/skill:") ? "skill" : "command";
}

/** The chip's colour tone — one map feeds the badge (transcript + floating) and
 *  the inline input highlight, so both surfaces stay in lockstep. */
export const CHIP_TONE: Record<ChipCategory, string> = {
  skill: "bg-teal-100 text-teal-800 ring-teal-300",
  command: "bg-amber-100 text-amber-800 ring-amber-300",
};
