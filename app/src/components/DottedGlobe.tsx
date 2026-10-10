import { useEffect, useRef } from "react";
import {
  geoGraticule10,
  geoOrthographic,
  geoPath,
  type GeoPermissibleObjects,
} from "d3-geo";
import { feature } from "topojson-client";
import type { GeometryCollection, Topology } from "topojson-specification";
// Served from this site, fingerprinted and cached, rather than fetched from a third party.
import landUrl from "world-atlas/land-110m.json?url";

import { dotTrig, landDots, projectDots } from "@/lib/globe";

/** Degrees per second: half a degree per frame at 60 Hz, the original spin. */
const SPIN = 30;
/** Degrees of turn per pixel dragged. */
const DRAG = 0.5;
const TAU = Math.PI * 2;

/** Decoded once per page load, not once per mount. */
let trigCache: Float64Array | null = null;
const dots = () => (trigCache ??= dotTrig(landDots()));

/**
 * The landing page's globe: Natural Earth's land as a halftone of dots on a wireframe sphere,
 * turning slowly, and turned by hand when dragged.
 *
 * Drawn in layers, back to front: the black disc and its rim, the 10° graticule, the coastlines,
 * then the dots. The disc spans 80% of the square (radius = size / 2.5), so the placeholder on
 * the landing page can reserve exactly the right shape.
 *
 * The dots come precomputed from `lib/globe-dots.ts` and are projected with plain arithmetic, so
 * they appear the moment this chunk loads. The coastlines follow when the land file arrives;
 * if it never does, the globe is still a globe.
 *
 * There is no scroll-to-zoom: on a page that scrolls, a wheel handler over the hero would trap
 * the page. Touch drags turn it sideways while vertical swipes still scroll. It spins only while
 * on screen, and not at all under `prefers-reduced-motion`.
 */
export function DottedGlobe({ className = "" }: { className?: string }) {
  const box = useRef<HTMLDivElement | null>(null);
  const ref = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const hostEl = box.current;
    const el = ref.current;
    const context = el?.getContext("2d");
    if (!hostEl || !el || !context) return;
    const host: HTMLDivElement = hostEl;
    const canvas: HTMLCanvasElement = el;
    const ctx: CanvasRenderingContext2D = context;

    const trig = dots();
    const screen = new Float32Array(trig.length / 2);
    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    const projection = geoOrthographic().clipAngle(90);
    const path = geoPath(projection, ctx);
    const graticule = geoGraticule10();
    let land: GeoPermissibleObjects | null = null;

    let size = 0;
    const rotation: [number, number] = [0, 0];
    let dragging = false;
    let visible = true;
    let frameId = 0;
    let last = 0;
    let start = { x: 0, y: 0, lambda: 0, phi: 0 };

    const render = () => {
      if (!size) return;
      const radius = size / 2.5;
      const c = size / 2;
      projection.scale(radius).translate([c, c]).rotate(rotation);
      ctx.clearRect(0, 0, size, size);

      ctx.beginPath();
      ctx.arc(c, c, radius, 0, TAU);
      ctx.fillStyle = "#000000";
      ctx.fill();
      ctx.strokeStyle = "#ffffff";
      ctx.lineWidth = 2;
      ctx.stroke();

      ctx.beginPath();
      path(graticule);
      ctx.strokeStyle = "#ffffff";
      ctx.lineWidth = 1;
      ctx.globalAlpha = 0.25;
      ctx.stroke();
      ctx.globalAlpha = 1;

      if (land) {
        ctx.beginPath();
        path(land);
        ctx.strokeStyle = "#ffffff";
        ctx.lineWidth = 1;
        ctx.stroke();
      }

      // Every dot in one path and one fill, rather than a fill per dot.
      projectDots(trig, rotation[0], rotation[1], radius, c, c, screen);
      ctx.beginPath();
      for (let i = 0; i < screen.length; i += 2) {
        ctx.moveTo(screen[i] + 1.2, screen[i + 1]);
        ctx.arc(screen[i], screen[i + 1], 1.2, 0, TAU);
      }
      ctx.fillStyle = "#999999";
      ctx.fill();
    };

    const spinning = () => visible && !dragging && !still.matches;

    const frame = (now: number) => {
      const dt = last ? Math.min(now - last, 100) : 0;
      last = now;
      rotation[0] = (rotation[0] + (SPIN * dt) / 1000) % 360;
      render();
      if (spinning()) {
        frameId = requestAnimationFrame(frame);
      } else {
        frameId = 0;
        last = 0;
      }
    };

    const spin = () => {
      if (!frameId && spinning()) frameId = requestAnimationFrame(frame);
    };

    const resize = () => {
      const next = Math.round(host.clientWidth);
      if (!next || next === size) return;
      size = next;
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(size * dpr);
      canvas.height = Math.round(size * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      render();
    };

    const onDown = (e: PointerEvent) => {
      if (e.button !== 0) return;
      dragging = true;
      canvas.setPointerCapture(e.pointerId);
      start = {
        x: e.clientX,
        y: e.clientY,
        lambda: rotation[0],
        phi: rotation[1],
      };
    };

    const onMove = (e: PointerEvent) => {
      if (!dragging) return;
      rotation[0] = start.lambda + (e.clientX - start.x) * DRAG;
      rotation[1] = Math.max(
        -90,
        Math.min(90, start.phi - (e.clientY - start.y) * DRAG)
      );
      render();
    };

    const onUp = (e: PointerEvent) => {
      if (!dragging) return;
      dragging = false;
      if (canvas.hasPointerCapture(e.pointerId)) {
        canvas.releasePointerCapture(e.pointerId);
      }
      spin();
    };

    const sizer = new ResizeObserver(resize);
    sizer.observe(host);
    const watcher = new IntersectionObserver(([entry]) => {
      visible = entry?.isIntersecting ?? true;
      spin();
    });
    watcher.observe(host);
    still.addEventListener("change", spin);
    canvas.addEventListener("pointerdown", onDown);
    canvas.addEventListener("pointermove", onMove);
    canvas.addEventListener("pointerup", onUp);
    canvas.addEventListener("pointercancel", onUp);
    resize();
    spin();

    let cancelled = false;
    fetch(landUrl)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(r.statusText))))
      .then((topo: Topology<{ land: GeometryCollection }>) => {
        if (cancelled) return;
        land = feature(topo, topo.objects.land);
        render();
      })
      .catch(() => {
        // Coastlines are a layer, not the globe. The dots and graticule already show the world.
      });

    return () => {
      cancelled = true;
      cancelAnimationFrame(frameId);
      sizer.disconnect();
      watcher.disconnect();
      still.removeEventListener("change", spin);
      canvas.removeEventListener("pointerdown", onDown);
      canvas.removeEventListener("pointermove", onMove);
      canvas.removeEventListener("pointerup", onUp);
      canvas.removeEventListener("pointercancel", onUp);
    };
  }, []);

  return (
    <div ref={box} className={`relative aspect-square w-full ${className}`}>
      <canvas
        ref={ref}
        role="img"
        aria-label="A globe of the world's land drawn in dots, turning slowly. Drag to rotate it."
        className="block h-full w-full cursor-grab touch-pan-y active:cursor-grabbing"
      />
      <div className="pointer-events-none absolute bottom-4 left-4 rounded-md bg-surface px-2 py-1 text-xs text-ink-dim">
        Drag to rotate
      </div>
    </div>
  );
}
