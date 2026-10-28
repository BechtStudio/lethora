// The split page pane's width: a remembered fraction of the window, clamped
// so that the editor and the pane each keep a minimum width.

/** The narrowest the editor or the pane gets, in CSS pixels. */
export const SPLIT_MIN = 320;
/** How far a Left/Right arrow key moves the handle, as a fraction of the window. */
export const SPLIT_STEP = 0.05;

/** A stored fraction, or half the window if it is missing or out of range. */
export function parseFraction(stored: string | null): number {
  const f = stored === null ? NaN : Number(stored);
  return Number.isFinite(f) && f > 0 && f < 1 ? f : 0.5;
}

/** The pane's width for `fraction` of `total` pixels; half each when both minimums do not fit. */
export function paneWidth(fraction: number, total: number, min = SPLIT_MIN): number {
  if (total < 2 * min) return total / 2;
  const f = Number.isFinite(fraction) ? fraction : 0.5;
  return Math.min(total - min, Math.max(min, f * total));
}

/** `fraction`, clamped as `paneWidth` would (half without a width). */
export function clampFraction(fraction: number, total: number, min = SPLIT_MIN): number {
  return total > 0 ? paneWidth(fraction, total, min) / total : 0.5;
}

/** The fraction for the handle at `x` (the pane is to its right), clamped. */
export function fractionAt(x: number, total: number, min = SPLIT_MIN): number {
  return total > 0 ? clampFraction((total - x) / total, total, min) : 0.5;
}
