/**
 * The mark: a lattice of diamonds with short bars joining the large ones.
 *
 * Drawn rather than loaded. Three reasons, in order of how much they matter:
 *
 * 1. **It takes its colour from the text around it.** Every shape is `currentColor`, so the
 *    same component is purple inside `text-brand` on SolFX and cyan inside `text-brand` on
 *    NOXFUNDS — one mark, two products, no second asset and no chance of the two drifting.
 * 2. It is a few hundred bytes inline, with no extra request and nothing to go stale in a cache.
 * 3. It stays sharp at any size, which a raster mark in a sticky header does not.
 *
 * The geometry is a reconstruction from the supplied artwork rather than the original file. If
 * it is not exact, dropping the real SVG in and swapping this component out is a one-line change
 * at each call site.
 */

type Size = "lg" | "md" | "sm" | "xs";

const R: Record<Size, number> = { lg: 6, md: 4.5, sm: 3.5, xs: 2.5 };

/** One row of the lattice: its baseline, its diamonds, and which neighbours are joined. */
type Row = {
  y: number;
  at: [number, Size][];
  /** Pairs of x centres joined by a bar. */
  bars?: [number, number][];
};

const ROWS: Row[] = [
  { y: 8, at: [[65, "xs"]] },
  {
    y: 23,
    at: [
      [37, "lg"],
      [56, "lg"],
      [74, "lg"],
      [90, "sm"],
      [105, "xs"],
    ],
    bars: [
      [37, 56],
      [56, 74],
    ],
  },
  {
    y: 38,
    at: [
      [15, "xs"],
      [31, "md"],
      [47, "lg"],
    ],
    bars: [[31, 47]],
  },
  {
    y: 53,
    at: [
      [23, "xs"],
      [39, "sm"],
      [56, "lg"],
      [74, "lg"],
    ],
    bars: [[56, 74]],
  },
  {
    y: 68,
    at: [
      [46, "lg"],
      [64, "lg"],
      [81, "sm"],
      [96, "xs"],
    ],
    bars: [[46, 64]],
  },
  {
    y: 83,
    at: [
      [72, "lg"],
      [90, "md"],
      [105, "xs"],
    ],
    bars: [[72, 90]],
  },
  {
    y: 98,
    at: [
      [14, "xs"],
      [29, "sm"],
      [46, "lg"],
      [64, "lg"],
      [82, "lg"],
    ],
    bars: [
      [46, 64],
      [64, 82],
    ],
  },
  { y: 112, at: [[54, "xs"]] },
];

const diamond = (cx: number, cy: number, r: number) =>
  `M ${cx} ${cy - r} L ${cx + r} ${cy} L ${cx} ${cy + r} L ${cx - r} ${cy} Z`;

const BAR_HALF_HEIGHT = 1.2;

export function Logo({ className = "h-7 w-7" }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 120 120"
      className={className}
      fill="currentColor"
      aria-hidden
      focusable="false"
    >
      {ROWS.map((row) => (
        <g key={row.y}>
          {/* Bars first, so a diamond always paints over the joint rather than under it. */}
          {(row.bars ?? []).map(([a, b]) => (
            <rect
              key={`${a}-${b}`}
              x={a}
              y={row.y - BAR_HALF_HEIGHT}
              width={b - a}
              height={BAR_HALF_HEIGHT * 2}
            />
          ))}
          {row.at.map(([x, size]) => (
            <path key={`${x}-${size}`} d={diamond(x, row.y, R[size])} />
          ))}
        </g>
      ))}
    </svg>
  );
}

/**
 * The mark beside the wordmark, as both headers use it.
 *
 * `product` only changes the words. The colour comes from the accent the surrounding surface
 * has already set, which is the whole point of the arrangement.
 */
export function Wordmark({
  product,
  className = "",
}: {
  product: "solfx" | "nox";
  className?: string;
}) {
  return (
    <span className={`flex items-center gap-2.5 ${className}`}>
      <Logo className="h-7 w-7 shrink-0 text-brand" />
      {product === "solfx" ? (
        <span className="text-xl font-extrabold tracking-tighter">SOL-FX</span>
      ) : (
        <span className="text-lg font-extrabold uppercase tracking-tight md:text-xl">
          SOL-FX <span className="text-ink-dim">/</span>{" "}
          <span className="text-brand">NOXFUNDS</span>
        </span>
      )}
    </span>
  );
}
