import { useEffect, useRef } from "react";

import {
  GRID,
  advanceRipple,
  easeFactor,
  lerp,
  mix,
  parseChannels,
  pinFactor,
  rgba,
  smoothstep,
  warpPoint,
  type Rgb,
  type Ripple,
} from "@/lib/kinetic";

const WHITE: Rgb = [255, 255, 255];
/** Used only if the surface sets no hue: SolFX's purple. */
const FALLBACK: Rgb = [152, 67, 254];
/** Where the cursor is before it has ever moved: far enough away to influence nothing. */
const OFF = -9999;
/** Clicking faster than ripples fade should not pile up unbounded work per frame. */
const MAX_RIPPLES = 8;
const TAU = Math.PI * 2;

/** The parts that never move, the background fill and the faint dot texture, painted once per size. */
function paintTexture(
  width: number,
  height: number,
  dpr: number,
  background: string
): HTMLCanvasElement | null {
  const tex = document.createElement("canvas");
  tex.width = Math.round(width * dpr);
  tex.height = Math.round(height * dpr);
  const g = tex.getContext("2d");
  if (!g) return null;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = background;
  g.fillRect(0, 0, width, height);
  g.fillStyle = "rgba(255,255,255,0.05)";
  g.beginPath();
  for (let x = GRID.dotSpacing / 2; x < width; x += GRID.dotSpacing) {
    for (let y = GRID.dotSpacing / 2; y < height; y += GRID.dotSpacing) {
      g.moveTo(x + 0.7, y);
      g.arc(x, y, 0.7, 0, TAU);
    }
  }
  g.fill();
  return tex;
}

/**
 * The page backdrop: a grid that bends toward the cursor and ripples where you click.
 *
 * Fixed to the viewport, so one canvas covers any scroll height without a seam where a section
 * ends. It sits at `-z-10`, and every page that uses it sets `isolate` on its root, which keeps
 * the canvas above the root's own background and beneath everything the page contains.
 *
 * Its colours come from the surface it is placed in rather than from a prop. `--sf-grid-rgb` and
 * `--sf-ripple-rgb` are purple on SolFX and blue under `[data-product="nox"]`, the attribute that
 * already gives NOXFUNDS its accent, so neither product passes anything.
 *
 * It only draws while something is moving. Once the eased cursor has caught up and the last ripple
 * has faded, the loop stops until the next pointer move, click or resize, so an idle page costs
 * nothing. Under `prefers-reduced-motion` it shows the resting grid and never animates.
 */
export function KineticGrid() {
  const ref = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const el = ref.current;
    const context = el?.getContext("2d");
    if (!el || !context) return;
    const canvas: HTMLCanvasElement = el;
    const ctx: CanvasRenderingContext2D = context;

    const css = getComputedStyle(canvas);
    const hue = parseChannels(css.getPropertyValue("--sf-grid-rgb"), FALLBACK);
    const ring = parseChannels(css.getPropertyValue("--sf-ripple-rgb"), hue);
    const background = css.getPropertyValue("--sf-bg").trim() || "#161618";
    const still = window.matchMedia("(prefers-reduced-motion: reduce)");

    let width = 0;
    let height = 0;
    let cols = 0;
    let rows = 0;
    let texture: HTMLCanvasElement | null = null;
    let pins = new Float32Array(0);
    let xs = new Float32Array(0);
    let ys = new Float32Array(0);
    let near = new Float32Array(0);

    const mouse = { x: OFF, y: OFF };
    const target = { x: OFF, y: OFF };
    /** 0 to 1: how present the cursor is. Fades as it leaves the window and returns. */
    let presence = 0;
    let inside = false;
    const ripples: Ripple[] = [];
    let frameId = 0;
    let last = 0;

    const draw = () => {
      if (texture) {
        ctx.drawImage(texture, 0, 0, width, height);
      } else {
        ctx.fillStyle = background;
        ctx.fillRect(0, 0, width, height);
      }

      const cellW = width / (cols - 1);
      const cellH = height / (rows - 1);
      for (let row = 0; row < rows; row++) {
        for (let col = 0; col < cols; col++) {
          const i = row * cols + col;
          const w = warpPoint(
            col * cellW,
            row * cellH,
            pins[i],
            mouse.x,
            mouse.y,
            presence,
            ripples
          );
          xs[i] = w.x;
          ys[i] = w.y;
          near[i] = w.proximity;
        }
      }

      // Lines. Everything at rest goes into one path and one stroke; only the handful of segments
      // near the cursor are drawn one by one, because each has its own colour and width.
      ctx.lineCap = "butt";
      const hot: number[] = [];
      ctx.beginPath();
      const segment = (a: number, b: number) => {
        if (near[a] + near[b] > 0) {
          hot.push(a, b);
          return;
        }
        ctx.moveTo(xs[a], ys[a]);
        ctx.lineTo(xs[b], ys[b]);
      };
      for (let row = 0; row < rows; row++) {
        for (let col = 0; col < cols - 1; col++) {
          segment(row * cols + col, row * cols + col + 1);
        }
      }
      for (let col = 0; col < cols; col++) {
        for (let row = 0; row < rows - 1; row++) {
          segment(row * cols + col, (row + 1) * cols + col);
        }
      }
      ctx.strokeStyle = rgba(WHITE, 0.13);
      ctx.lineWidth = 0.8;
      ctx.stroke();
      for (let k = 0; k < hot.length; k += 2) {
        const a = hot[k];
        const b = hot[k + 1];
        const t = smoothstep((near[a] + near[b]) / 2);
        ctx.beginPath();
        ctx.moveTo(xs[a], ys[a]);
        ctx.lineTo(xs[b], ys[b]);
        ctx.strokeStyle = mix(WHITE, 0.13, hue, 0.9, t);
        ctx.lineWidth = lerp(0.8, 1.5, t);
        ctx.stroke();
      }

      // Nodes, batched the same way.
      const hotNodes: number[] = [];
      ctx.beginPath();
      for (let i = 0; i < cols * rows; i++) {
        if (near[i] > 0) {
          hotNodes.push(i);
          continue;
        }
        ctx.moveTo(xs[i] + 1.8, ys[i]);
        ctx.arc(xs[i], ys[i], 1.8, 0, TAU);
      }
      ctx.fillStyle = rgba(WHITE, 0.2);
      ctx.fill();
      for (const i of hotNodes) {
        const t = smoothstep(near[i]);
        const r = lerp(1.8, 3.2, t);
        if (t > 0.3) {
          const glowR = r + lerp(0, 6, (t - 0.3) / 0.7);
          const grd = ctx.createRadialGradient(
            xs[i],
            ys[i],
            r * 0.5,
            xs[i],
            ys[i],
            glowR
          );
          grd.addColorStop(0, rgba(hue, t * 0.3));
          grd.addColorStop(1, rgba(hue, 0));
          ctx.beginPath();
          ctx.arc(xs[i], ys[i], glowR, 0, TAU);
          ctx.fillStyle = grd;
          ctx.fill();
        }
        ctx.beginPath();
        ctx.arc(xs[i], ys[i], r, 0, TAU);
        ctx.fillStyle = mix(WHITE, 0.2, hue, 1, t);
        ctx.fill();
      }

      for (const r of ripples) {
        ctx.beginPath();
        ctx.arc(r.x, r.y, Math.max(0, r.radius), 0, TAU);
        ctx.strokeStyle = rgba(ring, r.opacity * 0.28);
        ctx.lineWidth = 1.5;
        ctx.stroke();
      }
    };

    const frame = (now: number) => {
      const dt = last ? Math.min(now - last, 100) : 1000 / 60;
      last = now;
      const k = easeFactor(GRID.lerp, dt);
      mouse.x = lerp(mouse.x, target.x, k);
      mouse.y = lerp(mouse.y, target.y, k);
      const goal = inside ? 1 : 0;
      presence = lerp(presence, goal, k);
      for (let i = ripples.length - 1; i >= 0; i--) {
        if (!advanceRipple(ripples[i], now)) ripples.splice(i, 1);
      }

      const settled =
        Math.abs(mouse.x - target.x) < 0.25 &&
        Math.abs(mouse.y - target.y) < 0.25 &&
        Math.abs(presence - goal) < 0.005 &&
        ripples.length === 0;
      if (settled) {
        mouse.x = target.x;
        mouse.y = target.y;
        presence = goal;
      }
      draw();
      if (settled) {
        frameId = 0;
        last = 0;
      } else {
        frameId = requestAnimationFrame(frame);
      }
    };

    const wake = () => {
      if (!frameId) frameId = requestAnimationFrame(frame);
    };

    const resize = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      width = window.innerWidth;
      height = window.innerHeight;
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(height * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      cols = Math.max(2, Math.ceil(width / GRID.cell)) + 1;
      rows = Math.max(2, Math.ceil(height / GRID.cell)) + 1;
      const n = cols * rows;
      pins = new Float32Array(n);
      xs = new Float32Array(n);
      ys = new Float32Array(n);
      near = new Float32Array(n);
      for (let row = 0; row < rows; row++) {
        for (let col = 0; col < cols; col++) {
          pins[row * cols + col] = pinFactor(col, row, cols, rows);
        }
      }
      texture = paintTexture(width, height, dpr, background);
      draw();
    };

    const onMove = (e: PointerEvent) => {
      // A finger has no hover, and following it would warp the grid on every scroll.
      if (e.pointerType === "touch" || still.matches) return;
      target.x = e.clientX;
      target.y = e.clientY;
      // Coming back after fading out: start where the pointer is, not where it left.
      if (!inside && presence < 0.01) {
        mouse.x = target.x;
        mouse.y = target.y;
      }
      inside = true;
      wake();
    };

    const onOut = (e: MouseEvent) => {
      if (e.relatedTarget !== null) return;
      inside = false;
      wake();
    };

    const onClick = (e: MouseEvent) => {
      // `detail` is 0 for a click synthesised from the keyboard, which has no place on screen.
      if (e.button !== 0 || e.detail === 0 || still.matches) return;
      if (ripples.length >= MAX_RIPPLES) ripples.shift();
      ripples.push({
        x: e.clientX,
        y: e.clientY,
        born: performance.now(),
        radius: 0,
        opacity: 1,
      });
      wake();
    };

    const onMotionPreference = () => {
      ripples.length = 0;
      inside = false;
      presence = 0;
      mouse.x = target.x = OFF;
      mouse.y = target.y = OFF;
      draw();
    };

    resize();
    window.addEventListener("resize", resize);
    window.addEventListener("pointermove", onMove, { passive: true });
    window.addEventListener("mouseout", onOut);
    window.addEventListener("click", onClick);
    still.addEventListener("change", onMotionPreference);

    return () => {
      cancelAnimationFrame(frameId);
      window.removeEventListener("resize", resize);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("mouseout", onOut);
      window.removeEventListener("click", onClick);
      still.removeEventListener("change", onMotionPreference);
    };
  }, []);

  return (
    <canvas
      ref={ref}
      aria-hidden
      className="pointer-events-none fixed inset-0 -z-10 h-full w-full"
    />
  );
}
