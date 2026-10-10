import { geoOrthographic } from "d3-geo";
import { describe, expect, it } from "vitest";

import { dotTrig, landDots, projectDots } from "@/lib/globe";
import { LAND_COLS, LAND_DOT_COUNT, LAND_ROWS } from "@/lib/globe-dots";

const dots = landDots();
const step = 360 / LAND_COLS;

/** Lattice cells that are land, keyed "row,col". */
const land = new Set<string>();
for (let i = 0; i < dots.length; i += 2) {
  const col = Math.round((dots[i] + 180) / step - 0.5);
  const row = Math.round((dots[i + 1] + 90) / step - 0.5);
  land.add(`${row},${col}`);
}
const isLand = (lng: number, lat: number) =>
  land.has(
    `${Math.floor((lat + 90) / step)},${Math.floor((lng + 180) / step)}`
  );

/**
 * The bits are generated, so the test is of the data that actually ships: real places land on
 * the right side of the coastline, and the proportion is what land on a latitude/longitude
 * lattice should be. Polygons wound the wrong way would invert the globe, putting dots on every
 * ocean and two thirds of the lattice on "land".
 */
describe("the generated land dots", () => {
  it("decode to exactly the count the generator recorded", () => {
    expect(dots.length / 2).toBe(LAND_DOT_COUNT);
  });

  it("put land and sea where they are", () => {
    expect(isLand(-0.13, 51.5)).toBe(true); // London
    expect(isLand(10, 23)).toBe(true); // Sahara
    expect(isLand(-60, -10)).toBe(true); // Amazon
    expect(isLand(135, -25)).toBe(true); // central Australia
    expect(isLand(0, -85)).toBe(true); // Antarctica, enclosing the pole
    expect(isLand(-30, 0)).toBe(false); // mid-Atlantic
    expect(isLand(-150, 0)).toBe(false); // mid-Pacific
    expect(isLand(80, -20)).toBe(false); // Indian Ocean
  });

  it("cover about a third of the lattice, not two thirds", () => {
    // A third rather than the 29% of the sphere that is land, because a lat/lng lattice packs
    // its points closer together toward the poles, where Antarctica and the Arctic coasts are.
    const share = LAND_DOT_COUNT / (LAND_COLS * LAND_ROWS);
    expect(share).toBeGreaterThan(0.3);
    expect(share).toBeLessThan(0.36);
  });
});

describe("projectDots matches d3's orthographic projection", () => {
  it("agrees with geoOrthographic for random points and rotations, far side included", () => {
    // A fixed linear congruential sequence, so a failure reproduces.
    let seed = 12345;
    const rand = () => {
      seed = (seed * 1103515245 + 12345) % 2 ** 31;
      return seed / 2 ** 31;
    };

    const scale = 224;
    const cx = 280;
    const cy = 280;
    const sample = new Float64Array(400);
    for (let i = 0; i < sample.length; i += 2) {
      sample[i] = rand() * 360 - 180;
      sample[i + 1] = rand() * 180 - 90;
    }
    const trig = dotTrig(sample);
    const out = new Float32Array(sample.length);

    for (let r = 0; r < 25; r++) {
      const lambda = rand() * 720 - 360;
      const phi = rand() * 180 - 90;
      projectDots(trig, lambda, phi, scale, cx, cy, out);
      const d3 = geoOrthographic()
        .scale(scale)
        .translate([cx, cy])
        .clipAngle(90)
        .rotate([lambda, phi]);
      for (let i = 0; i < sample.length; i += 2) {
        const expected = d3([sample[i], sample[i + 1]]);
        expect(expected).not.toBeNull();
        if (!expected) return;
        // Float32 output, so agreement to a thousandth of a pixel is the honest bound.
        expect(out[i]).toBeCloseTo(expected[0], 3);
        expect(out[i + 1]).toBeCloseTo(expected[1], 3);
      }
    }
  });
});
