import { useEffect, useState, type ReactNode } from "react";

/** How far the page has to scroll before the header frosts over, in pixels. */
const THRESHOLD = 8;

const scrolledNow = () =>
  typeof window !== "undefined" && window.scrollY > THRESHOLD;

/**
 * A header that is clear while the page sits at the top and turns to frosted glass once it
 * scrolls, so the page and the grid behind it blur as they slide underneath.
 *
 * One passive scroll listener. It only changes state when the page crosses the threshold, so
 * scrolling does not re-render the header every frame. The look itself is `.glass-header` in
 * `index.css`, keyed off `data-scrolled`.
 *
 * `ruled` keeps the bottom border at rest. The terminal never scrolls, and its header is
 * separated from the panels below by that line.
 */
export function GlassHeader({
  className = "",
  ruled = false,
  children,
}: {
  className?: string;
  ruled?: boolean;
  children: ReactNode;
}) {
  const [scrolled, setScrolled] = useState(scrolledNow);

  useEffect(() => {
    const onScroll = () => setScrolled(scrolledNow());
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  return (
    <header
      data-scrolled={scrolled ? "" : undefined}
      className={`glass-header ${ruled ? "glass-header-ruled" : ""} ${className}`}
    >
      {children}
    </header>
  );
}
