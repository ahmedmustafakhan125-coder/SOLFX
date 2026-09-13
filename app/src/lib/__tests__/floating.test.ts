import { afterEach, describe, expect, it, vi } from "vitest";

import { clampRect, defaultRect } from "@/lib/floating";

/** The environment is `node`, so the viewport has to be supplied. */
function viewport(w: number, h: number) {
  vi.stubGlobal("window", { innerWidth: w, innerHeight: h });
}

afterEach(() => vi.unstubAllGlobals());

/**
 * The property that matters for a draggable window is that it can always be dragged back.
 * A window parked past the right edge, or above the top of the screen, is a window a trader
 * has to clear site data to recover — so the clamp is the whole safety story here.
 */
describe("clampRect keeps the window reachable", () => {
  it("never lets the title bar leave the top of the screen", () => {
    viewport(1600, 900);
    expect(clampRect({ x: 100, y: -500, w: 600, h: 300 }).y).toBe(0);
  });

  it("never lets the window sit below the bottom edge", () => {
    viewport(1600, 900);
    expect(clampRect({ x: 100, y: 5_000, w: 600, h: 300 }).y).toBe(860);
  });

  it("leaves a grab strip on screen at both horizontal extremes", () => {
    viewport(1600, 900);
    const w = 600;
    const right = clampRect({ x: 9_999, y: 10, w, h: 300 });
    expect(right.x).toBe(1600 - 96);

    const left = clampRect({ x: -9_999, y: 10, w, h: 300 });
    // At least 96px of the window's right edge stays past x = 0.
    expect(left.x + w).toBeGreaterThanOrEqual(96);
  });

  it("enforces a minimum size, so it cannot be resized into nothing", () => {
    viewport(1600, 900);
    const r = clampRect({ x: 10, y: 10, w: 1, h: 1 });
    expect(r.w).toBe(380);
    expect(r.h).toBe(160);
  });

  it("does not force a minimum larger than the screen", () => {
    // A phone in portrait is narrower than MIN_W. Clamping up to 380 and then clamping x
    // must still leave the window placeable rather than producing a negative width.
    viewport(360, 740);
    const r = clampRect({ x: 0, y: 0, w: 1, h: 1 });
    expect(r.w).toBe(380);
    expect(r.x).toBeLessThanOrEqual(360 - 96);
  });

  it("is idempotent — clamping a clamped rect changes nothing", () => {
    viewport(1440, 810);
    const once = clampRect({ x: 3_000, y: -20, w: 20_000, h: 4 });
    expect(clampRect(once)).toEqual(once);
  });
});

describe("defaultRect", () => {
  it("lands inside the viewport and survives its own clamp", () => {
    for (const [w, h] of [
      [1920, 1080],
      [1440, 900],
      [1280, 720],
      [390, 844],
    ] as const) {
      viewport(w, h);
      const r = defaultRect();
      expect(clampRect(r)).toEqual(r);
      expect(r.y).toBeGreaterThanOrEqual(0);
    }
  });
});
