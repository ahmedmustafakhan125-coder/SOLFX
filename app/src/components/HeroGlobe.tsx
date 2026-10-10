import { Suspense, lazy } from "react";

/** The globe and its geometry library load after the page, so they never hold up first paint. */
const DottedGlobe = lazy(() =>
  import("@/components/DottedGlobe").then((m) => ({ default: m.DottedGlobe }))
);

/**
 * Holds the globe's place while its chunk loads: the same square, with the disc's outline where
 * the disc will be (80% of the box, matching `DottedGlobe`), so nothing moves when it arrives.
 */
function GlobePlaceholder() {
  return (
    <div className="relative aspect-square w-full">
      <div className="absolute inset-[10%] rounded-full border-2 border-white/10" />
    </div>
  );
}

/**
 * The dotted globe as both landing pages show it, SolFX's and NOXFUNDS'. One wrapper, so the
 * lazy load and its placeholder are written once.
 */
export function HeroGlobe() {
  return (
    <Suspense fallback={<GlobePlaceholder />}>
      <DottedGlobe />
    </Suspense>
  );
}
