import { describe, expect, it } from "vitest";

import {
  GRID,
  advanceRipple,
  easeFactor,
  parseChannels,
  pinFactor,
  warpPoint,
  type Ripple,
} from "@/lib/kinetic";

const PURPLE = [152, 67, 254] as const;

/**
 * The backdrop must never move what it frames, and must never throw a node somewhere absurd.
 * Those two properties, the pinned edges and the bounded pull, are what make it safe to put
 * behind every page.
 */
describe("the grid's edges stay pinned", () => {
  it("holds the outermost rows and columns completely still", () => {
    const cols = 28;
    const rows = 18;
    for (let col = 0; col < cols; col++) {
      expect(pinFactor(col, 0, cols, rows)).toBe(0);
      expect(pinFactor(col, rows - 1, cols, rows)).toBe(0);
    }
    for (let row = 0; row < rows; row++) {
      expect(pinFactor(0, row, cols, rows)).toBe(0);
      expect(pinFactor(cols - 1, row, cols, rows)).toBe(0);
    }
  });

  it("is fully free in the interior", () => {
    expect(pinFactor(10, 8, 28, 18)).toBe(1);
  });

  it("does not move a pinned node even with the cursor right beside it", () => {
    const w = warpPoint(0, 100, 0, 10, 100, 1, []);
    expect(w).toEqual({ x: 0, y: 100, proximity: 0 });
  });
});

describe("the cursor's pull is local and bounded", () => {
  it("leaves nodes beyond its reach where they are", () => {
    const w = warpPoint(500, 500, 1, 500 + GRID.influence + 1, 500, 1, []);
    expect(w).toEqual({ x: 500, y: 500, proximity: 0 });
  });

  it("pulls a nearby node toward the cursor, never past the maximum", () => {
    const mouse = { x: 400, y: 300 };
    for (let d = 1; d < GRID.influence; d += 7) {
      for (const [gx, gy] of [
        [mouse.x + d, mouse.y],
        [mouse.x, mouse.y - d],
        [mouse.x - d * 0.6, mouse.y + d * 0.8],
      ]) {
        const w = warpPoint(gx, gy, 1, mouse.x, mouse.y, 1, []);
        const moved = Math.hypot(w.x - gx, w.y - gy);
        expect(moved).toBeLessThanOrEqual(GRID.maxWarp);
        // Toward the cursor, and never overshooting it.
        const before = Math.hypot(gx - mouse.x, gy - mouse.y);
        const after = Math.hypot(w.x - mouse.x, w.y - mouse.y);
        expect(after).toBeLessThanOrEqual(before);
        expect(w.proximity).toBeGreaterThan(0);
        expect(w.proximity).toBeLessThanOrEqual(1);
      }
    }
  });

  it("has no effect at all once the pointer has left the window", () => {
    const w = warpPoint(420, 300, 1, 400, 300, 0, []);
    expect(w).toEqual({ x: 420, y: 300, proximity: 0 });
  });
});

describe("ripples", () => {
  const born = 1_000;
  const ripple = (): Ripple => ({ x: 0, y: 0, born, radius: 0, opacity: 1 });

  it("travel at their stated speed and fade out", () => {
    const r = ripple();
    expect(advanceRipple(r, born + 500)).toBe(true);
    expect(r.radius).toBeCloseTo(GRID.ripple.speed / 2);
    // A millisecond past the computed end, so floating-point rounding cannot decide the result.
    expect(advanceRipple(r, born + 1000 / GRID.ripple.fade + 1)).toBe(false);
    expect(r.opacity).toBe(0);
  });

  it("never get a negative radius from a frame stamped before the click", () => {
    const r = ripple();
    expect(advanceRipple(r, born - 5)).toBe(true);
    expect(r.radius).toBe(0);
  });

  it("only move nodes near their ring, by a bounded amount", () => {
    const r = ripple();
    advanceRipple(r, born + 250); // radius 100
    const onRing = warpPoint(100, 0, 1, -9999, -9999, 0, [r]);
    expect(Math.hypot(onRing.x - 100, onRing.y)).toBeLessThanOrEqual(
      GRID.ripple.strength
    );
    const farOutside = warpPoint(300, 0, 1, -9999, -9999, 0, [r]);
    expect(farOutside).toEqual({ x: 300, y: 0, proximity: 0 });
  });
});

describe("easing is independent of frame rate", () => {
  it("matches the design's per-frame rate at 60 Hz", () => {
    expect(easeFactor(GRID.lerp, 1000 / 60)).toBeCloseTo(GRID.lerp);
  });

  it("covers the same ground in two 120 Hz frames as in one 60 Hz frame", () => {
    const k120 = easeFactor(GRID.lerp, 1000 / 120);
    const twoFrames = 1 - (1 - k120) * (1 - k120);
    expect(twoFrames).toBeCloseTo(GRID.lerp);
  });
});

describe("parseChannels reads the hue a surface sets", () => {
  it("accepts the space- and comma-separated forms", () => {
    expect(parseChannels("152 67 254", [0, 0, 0])).toEqual(PURPLE);
    expect(parseChannels(" 74, 158 , 255 ", [0, 0, 0])).toEqual([74, 158, 255]);
  });

  it("falls back when the property is missing or malformed", () => {
    expect(parseChannels("", PURPLE)).toEqual(PURPLE);
    expect(parseChannels("purple", PURPLE)).toEqual(PURPLE);
    expect(parseChannels("1 2 300", PURPLE)).toEqual(PURPLE);
    expect(parseChannels("1 2", PURPLE)).toEqual(PURPLE);
  });
});
