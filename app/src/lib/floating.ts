/**
 * Geometry for the floating activity window.
 *
 * Separate from the component only because a module that exports both a component and plain
 * functions defeats React Fast Refresh. It earns its keep anyway: these are pure over their
 * inputs apart from reading the viewport, so the clamping rules are testable.
 */
export type Rect = {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
};

const MIN_W = 380;
const MIN_H = 160;

/**
 * A window can never be dragged somewhere it cannot be dragged back from: this much of it
 * stays on screen horizontally, and the title bar never goes above the top edge.
 */
const KEEP_VISIBLE = 96;

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

export function clampRect(r: Rect): Rect {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const w = clamp(r.w, MIN_W, Math.max(MIN_W, vw));
  const h = clamp(r.h, MIN_H, Math.max(MIN_H, vh));
  return {
    w,
    h,
    x: clamp(r.x, KEEP_VISIBLE - w, Math.max(0, vw - KEEP_VISIBLE)),
    y: clamp(r.y, 0, Math.max(0, vh - 40)),
  };
}

/** A sensible first position: lower-right, roughly half the viewport. */
export function defaultRect(): Rect {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const w = clamp(Math.round(vw * 0.62), MIN_W, 1100);
  const h = clamp(Math.round(vh * 0.42), MIN_H, 620);
  return clampRect({ w, h, x: vw - w - 24, y: vh - h - 24 });
}
