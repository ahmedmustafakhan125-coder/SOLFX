/**
 * The dotted globe's arithmetic: which points are land, and where each lands on screen.
 *
 * Geometry for a decoration, so floating point throughout and no money path near it.
 */
import { LAND_BITS, LAND_COLS, LAND_ROWS } from "@/lib/globe-dots";

const RAD = Math.PI / 180;

/**
 * Every land point of the generated lattice, as interleaved [longitude, latitude] in degrees.
 * See `scripts/globe-dots.mts` for how the bits were made.
 */
export function landDots(
  bits: string = LAND_BITS,
  cols: number = LAND_COLS,
  rows: number = LAND_ROWS
): Float64Array {
  const step = 360 / cols;
  const bytes = Uint8Array.from(atob(bits), (c) => c.charCodeAt(0));
  const out: number[] = [];
  for (let i = 0; i < cols * rows; i++) {
    if (bytes[i >> 3] & (1 << (i & 7))) {
      const row = Math.floor(i / cols);
      const col = i % cols;
      out.push(-180 + (col + 0.5) * step, -90 + (row + 0.5) * step);
    }
  }
  return Float64Array.from(out);
}

/**
 * The trigonometry of each dot, computed once, so a frame is multiplications only:
 * [cos lng, sin lng, cos lat, sin lat] per dot.
 */
export function dotTrig(lngLat: Float64Array): Float64Array {
  const out = new Float64Array(lngLat.length * 2);
  for (let i = 0, j = 0; i < lngLat.length; i += 2, j += 4) {
    const lng = lngLat[i] * RAD;
    const lat = lngLat[i + 1] * RAD;
    out[j] = Math.cos(lng);
    out[j + 1] = Math.sin(lng);
    out[j + 2] = Math.cos(lat);
    out[j + 3] = Math.sin(lat);
  }
  return out;
}

/**
 * Projects every dot orthographically, as d3's `geoOrthographic().rotate([lambda, phi])` with the
 * given scale and centre would, writing interleaved screen x, y into `out`.
 *
 * The same answer as calling the d3 projection per point (a test holds the two together), at a
 * fraction of the cost: d3 runs inverse trigonometry for every point, and this needs only the
 * angle-sum identities on precomputed values. Like d3's `projection(point)`, it does not clip, so
 * points on the far side land inside the disc too. That is the see-through look of the design.
 */
export function projectDots(
  trig: Float64Array,
  lambda: number,
  phi: number,
  scale: number,
  cx: number,
  cy: number,
  out: Float32Array
): void {
  const cl = Math.cos(lambda * RAD);
  const sl = Math.sin(lambda * RAD);
  const cp = Math.cos(phi * RAD);
  const sp = Math.sin(phi * RAD);
  for (let j = 0, k = 0; j < trig.length; j += 4, k += 2) {
    const cosLng = trig[j];
    const sinLng = trig[j + 1];
    const cosLat = trig[j + 2];
    const sinLat = trig[j + 3];
    // Longitude rotated by lambda, then the whole sphere tilted by phi.
    const x = (cosLng * cl - sinLng * sl) * cosLat;
    const y = (sinLng * cl + cosLng * sl) * cosLat;
    out[k] = cx + scale * y;
    out[k + 1] = cy - scale * (sinLat * cp + x * sp);
  }
}
