/**
 * The arithmetic behind the page backdrop's kinetic grid, kept apart from the canvas so it can be
 * tested without one.
 *
 * This is geometry for a background animation: floating point throughout, and nowhere near a
 * money path. The constants are the ones the design was chosen with, so the look does not drift
 * when the drawing code changes.
 */

/** Bare RGB channels, each 0 to 255. */
export type Rgb = readonly [number, number, number];

export const GRID = {
  /** Target cell size in CSS pixels. The real cells stretch so the grid fills the viewport exactly. */
  cell: 55,
  /** How far from the cursor the grid bends, in pixels. */
  influence: 260,
  /** The furthest a node is pulled toward the cursor, in pixels. */
  maxWarp: 24,
  /**
   * Within this many pixels of the cursor the pull eases off to nothing, so the node directly
   * under the pointer stays put instead of snapping onto it.
   */
  core: 60,
  /** Spacing of the faint static dot texture behind the grid. */
  dotSpacing: 28,
  /** Fraction of the remaining distance the eased cursor covers in one 60 Hz frame. */
  lerp: 0.08,
  /** Rows and columns this close to an edge are pinned, so the frame itself never moves. */
  edgeMargin: 1.5,
  /** A click sends out a ring: its speed, how fast it fades, the band it moves, and how hard. */
  ripple: { speed: 400, fade: 1.2, width: 55, strength: 18 },
} as const;

export type Ripple = {
  readonly x: number;
  readonly y: number;
  /** `performance.now()` at the click. */
  readonly born: number;
  radius: number;
  opacity: number;
};

export type Warped = { x: number; y: number; proximity: number };

export const lerp = (a: number, b: number, t: number): number =>
  a + (b - a) * t;

/** Eases 0 to 1 with zero slope at both ends. */
export const smoothstep = (t: number): number => t * t * (3 - 2 * t);

/**
 * A per-frame easing rate rescaled to the time that actually passed, so the grid feels the same
 * on a 120 Hz screen as on a 60 Hz one instead of catching up twice as fast.
 */
export function easeFactor(rate: number, dtMs: number): number {
  return 1 - Math.pow(1 - rate, dtMs / (1000 / 60));
}

/** 1 in the interior, falling to 0 on the outermost rows and columns. */
export function pinFactor(
  col: number,
  row: number,
  cols: number,
  rows: number
): number {
  const m = GRID.edgeMargin;
  const c = Math.min(col / m, (cols - 1 - col) / m, 1);
  const r = Math.min(row / m, (rows - 1 - row) / m, 1);
  return c * c * r * r;
}

/**
 * Ages a ripple to `now`. Returns false once it has faded out and should be dropped.
 *
 * Age is clamped at zero because a frame's timestamp can be earlier than the click handler's
 * `performance.now()` in the same frame, which would otherwise give a negative radius.
 */
export function advanceRipple(r: Ripple, now: number): boolean {
  const age = Math.max(0, (now - r.born) / 1000);
  r.radius = age * GRID.ripple.speed;
  r.opacity = Math.max(0, 1 - age * GRID.ripple.fade);
  return r.opacity > 0;
}

/**
 * Where a grid node at rest position (gx, gy) is drawn this frame, and how close it is to the
 * cursor (0 to 1, which drives its colour).
 *
 * Two forces. Every live ripple compresses the nodes near its ring toward the ring. The cursor
 * pulls nodes toward itself along a bell: nothing at the cursor, strongest a little way out,
 * nothing beyond `influence`. `pin` holds the edges still and `presence` fades the cursor's pull
 * in and out as the pointer enters and leaves the window.
 */
export function warpPoint(
  gx: number,
  gy: number,
  pin: number,
  mouseX: number,
  mouseY: number,
  presence: number,
  ripples: readonly Ripple[]
): Warped {
  let rx = 0;
  let ry = 0;
  if (pin > 0) {
    for (const r of ripples) {
      const rdx = gx - r.x;
      const rdy = gy - r.y;
      const rdist = Math.hypot(rdx, rdy);
      const diff = rdist - r.radius;
      if (rdist > 0 && Math.abs(diff) < GRID.ripple.width) {
        const strength =
          (1 - Math.abs(diff) / GRID.ripple.width) *
          r.opacity *
          GRID.ripple.strength *
          pin;
        // Inside the ring a node is pushed outward, outside it pulled inward: both toward the ring.
        const sign = diff < 0 ? -1 : 1;
        rx -= (rdx / rdist) * strength * sign;
        ry -= (rdy / rdist) * strength * sign;
      }
    }
  }

  const dx = gx - mouseX;
  const dy = gy - mouseY;
  const dist = Math.hypot(dx, dy);
  const proximity = Math.max(0, 1 - dist / GRID.influence) * pin * presence;

  if (dist > 0 && dist < GRID.influence && pin > 0 && presence > 0) {
    const t = dist / GRID.influence;
    const bell =
      t < 0.01 ? 0 : (1 - t) * (1 - t) * Math.min(1, dist / GRID.core);
    const pull = bell * GRID.maxWarp * pin * presence;
    return {
      x: gx - (dx / dist) * pull + rx,
      y: gy - (dy / dist) * pull + ry,
      proximity,
    };
  }
  return { x: gx + rx, y: gy + ry, proximity };
}

/** Reads "152 67 254" (spaces or commas) from a custom property. Anything else gives `fallback`. */
export function parseChannels(value: string, fallback: Rgb): Rgb {
  const parts = value
    .trim()
    .split(/[\s,]+/)
    .map(Number);
  const valid =
    parts.length === 3 &&
    parts.every((n) => Number.isFinite(n) && n >= 0 && n <= 255);
  return valid ? [parts[0], parts[1], parts[2]] : fallback;
}

export function rgba([r, g, b]: Rgb, alpha: number): string {
  return `rgba(${r},${g},${b},${alpha.toFixed(3)})`;
}

/** Channel by channel from one colour to another, alpha included. */
export function mix(
  from: Rgb,
  fromAlpha: number,
  to: Rgb,
  toAlpha: number,
  t: number
): string {
  return rgba(
    [
      Math.round(lerp(from[0], to[0], t)),
      Math.round(lerp(from[1], to[1], t)),
      Math.round(lerp(from[2], to[2], t)),
    ],
    lerp(fromAlpha, toAlpha, t)
  );
}
