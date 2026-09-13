import { useCallback, useEffect, useRef, type ReactNode } from "react";

import { clampRect, defaultRect, type Rect } from "@/lib/floating";

/**
 * A panel the trader can pull off the page, move and resize.
 *
 * The activity panel lives under the chart in a fixed-height terminal, which is the right
 * default — but it means a long positions table is read through a letterbox, and the
 * summary of a position is exactly what someone wants a clear look at while the chart
 * moves. Floating it is the cheapest fix that does not cost the docked layout anything.
 *
 * No drag library. This is two pointer handlers and a clamp; a dependency here would be more
 * code shipped than written, and the CSP on the deployed app only allows scripts from a
 * short CDN allowlist anyway.
 *
 * Pointer events rather than mouse events, so a trackpad, a touchscreen and a pen all work
 * from one path. `setPointerCapture` on the handle is what makes a fast drag keep tracking
 * after the cursor leaves the 12-pixel grip.
 */
type Drag = {
  readonly mode: "move" | "resize";
  readonly startX: number;
  readonly startY: number;
  readonly base: Rect;
};

export function FloatingPanel({
  title,
  rect,
  onRectChange,
  onDock,
  children,
}: {
  readonly title: string;
  readonly rect: Rect;
  readonly onRectChange: (r: Rect) => void;
  readonly onDock: () => void;
  readonly children: ReactNode;
}) {
  const drag = useRef<Drag | undefined>(undefined);

  // A window sized for a wide screen is off the edge of a narrow one. Re-clamping on resize
  // is what stops "it worked until I docked my laptop" being a bug report.
  useEffect(() => {
    const onResize = () => onRectChange(clampRect(rect));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [rect, onRectChange]);

  const begin = useCallback(
    (mode: Drag["mode"]) => (e: React.PointerEvent<HTMLElement>) => {
      // Left button and touch/pen only, and never when the press started on a control
      // inside the title bar — otherwise the dock button is a drag handle too.
      if (e.button !== 0) return;
      if ((e.target as HTMLElement).closest("button") && mode === "move")
        return;
      e.preventDefault();
      e.currentTarget.setPointerCapture(e.pointerId);
      drag.current = {
        mode,
        startX: e.clientX,
        startY: e.clientY,
        base: rect,
      };
    },
    [rect]
  );

  const move = useCallback(
    (e: React.PointerEvent<HTMLElement>) => {
      const d = drag.current;
      if (!d) return;
      const dx = e.clientX - d.startX;
      const dy = e.clientY - d.startY;
      onRectChange(
        clampRect(
          d.mode === "move"
            ? { ...d.base, x: d.base.x + dx, y: d.base.y + dy }
            : { ...d.base, w: d.base.w + dx, h: d.base.h + dy }
        )
      );
    },
    [onRectChange]
  );

  const end = useCallback((e: React.PointerEvent<HTMLElement>) => {
    if (!drag.current) return;
    drag.current = undefined;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  }, []);

  /** Arrow keys move it, with shift for a coarse step. A drag handle that needs a mouse
   *  is a drag handle half the people using this cannot reach. */
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const step = e.shiftKey ? 40 : 8;
      const by = (dx: number, dy: number) => {
        e.preventDefault();
        onRectChange(clampRect({ ...rect, x: rect.x + dx, y: rect.y + dy }));
      };
      if (e.key === "ArrowLeft") by(-step, 0);
      else if (e.key === "ArrowRight") by(step, 0);
      else if (e.key === "ArrowUp") by(0, -step);
      else if (e.key === "ArrowDown") by(0, step);
      else if (e.key === "Escape") onDock();
    },
    [rect, onRectChange, onDock]
  );

  return (
    <div
      role="dialog"
      aria-label={title}
      className="fixed z-40 flex flex-col overflow-hidden rounded-lg border border-line bg-surface shadow-2xl shadow-black/50"
      style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
    >
      <div
        role="toolbar"
        tabIndex={0}
        aria-label={`${title} — drag to move, arrow keys to nudge, Escape to dock`}
        onPointerDown={begin("move")}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
        onKeyDown={onKeyDown}
        onDoubleClick={() => onRectChange(defaultRect())}
        className="flex shrink-0 cursor-move touch-none select-none items-center justify-between gap-3 border-b border-line-soft bg-surface-high px-3 py-1.5 focus:outline-none focus-visible:ring-1 focus-visible:ring-brand"
      >
        <span className="text-[10px] uppercase tracking-[0.14em] text-ink-dim">
          {title}
        </span>
        <button
          onClick={onDock}
          title="Put it back under the chart (Esc)"
          className="rounded border border-line px-2 py-0.5 text-[11px] text-ink-muted hover:border-brand hover:text-ink"
        >
          Dock
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">{children}</div>

      <div
        role="slider"
        tabIndex={-1}
        aria-label="Resize"
        aria-valuenow={rect.w}
        onPointerDown={begin("resize")}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
        className="absolute bottom-0 right-0 h-4 w-4 cursor-nwse-resize touch-none"
      >
        {/* Two strokes, the conventional corner grip. Drawn rather than an icon font. */}
        <svg viewBox="0 0 16 16" className="h-4 w-4 text-ink-dim/70">
          <path
            d="M15 6 L6 15 M15 11 L11 15"
            stroke="currentColor"
            strokeWidth="1.2"
            fill="none"
          />
        </svg>
      </div>
    </div>
  );
}
