import { Link } from "react-router-dom";

/**
 * The strip above every header: two products, one surface.
 *
 * The active segment paints with `--color-brand`, which each surface redefines — purple on
 * SolFX, cyan on NOXFUNDS. So one component gives both sides their own identity without a
 * second copy or a colour prop, and the switch itself changes colour as you cross it.
 */
export function ProductSwitcher({ active }: { active: "solfx" | "nox" }) {
  const seg = (on: boolean) =>
    [
      "px-5 py-1.5 text-[11px] font-bold uppercase tracking-[0.18em] transition-colors",
      on
        ? "bg-brand text-[var(--sf-on-brand)]"
        : "text-ink-dim hover:text-ink-muted",
    ].join(" ");

  return (
    <div className="border-b border-line-soft bg-bg">
      <div className="mx-auto flex max-w-[1440px] items-center justify-center px-4 py-2">
        <div className="flex items-center rounded-full border border-line p-0.5">
          <Link
            to="/"
            className={`${seg(active === "solfx")} rounded-full`}
            aria-current={active === "solfx" ? "page" : undefined}
          >
            SOL-FX
          </Link>
          <Link
            to="/nox"
            className={`${seg(active === "nox")} rounded-full`}
            aria-current={active === "nox" ? "page" : undefined}
          >
            Noxfunds
          </Link>
        </div>
      </div>
    </div>
  );
}
