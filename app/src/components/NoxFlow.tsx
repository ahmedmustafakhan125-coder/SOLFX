import { useEffect, useRef, useState } from "react";

/**
 * The animated explainer for how a NOXFUNDS mandate works.
 *
 * Five steps, one scene. The geometry never moves between steps — only what is lit, and which
 * packet is in flight, changes. That is deliberate: a diagram that re-lays itself out on every
 * step makes the viewer re-read it five times instead of once.
 *
 * No animation library. Packets travel with CSS `offset-path`, which takes the same path data
 * the visible edge is drawn from, so a packet cannot drift away from its wire. Adding a
 * dependency to move six dots along six lines would be a poor trade.
 */

type StepId = "fund" | "order" | "check" | "mark" | "settle";

type Step = {
  id: StepId;
  n: number;
  chip: string;
  title: string;
  body: string;
  /** The one thing this step is evidence for. */
  point: string;
};

const STEPS: Step[] = [
  {
    id: "fund",
    n: 1,
    chip: "Fund",
    title: "An investor funds a mandate",
    body: "USDC leaves the investor's wallet and enters a vault controlled by a program address that has no private key: not the trader's, not the investor's, not the operator's. The rule set is written into the mandate at this moment.",
    point:
      "The rules are fixed at funding. No instruction in the program can edit them.",
  },
  {
    id: "order",
    n: 2,
    chip: "Order",
    title: "The trader signs NOXFUNDS, not the exchange",
    body: "The trader never holds the capital and never touches the venue. They sign an order to NOXFUNDS, which decides whether to forward it.",
    point: "The trader's signature never reaches the exchange.",
  },
  {
    id: "check",
    n: 3,
    chip: "Check",
    title: "Every rule is checked before the fill",
    body: "Position size, total exposure, permitted market, drawdown, hold time, and a stop-loss, which is mandatory and placed in the very same transaction. Only then is the order forwarded.",
    point:
      "A trade that breaks a rule does not get punished. The transaction reverts: no fill, no loss, no trace.",
  },
  {
    id: "mark",
    n: 4,
    chip: "Mark",
    title: "Anyone can mark the mandate to market",
    body: "A public crank prices every open position against the oracle and records the mandate's equity. Breach the drawdown limit and the mandate is frozen to new trades.",
    point:
      "The high-water mark only ever rises, so a trader cannot reset their drawdown by closing out.",
  },
  {
    id: "settle",
    n: 5,
    chip: "Settle",
    title: "The program divides the money",
    body: "5% of gross profit to the protocol, then 70/30 of what remains between trader and investor. On a loss there is no fee and no trader share. The investor receives everything left.",
    point: "Nobody approves the payout. There is nobody to be refused by.",
  },
];

/** Path data, shared between the drawn wire and the packet that travels it. */
const EDGE = {
  fund: "M 176 182 C 214 182 214 96 250 96",
  capital: "M 356 138 L 356 214",
  order: "M 176 334 L 248 330",
  pass: "M 462 296 L 558 284",
  reject: "M 462 360 C 508 408 250 436 176 362",
  mark: "M 652 214 C 600 168 520 146 462 122",
  settle: "M 462 86 L 558 104",
} as const;

/** The checks listed inside the rulebook node. The full set is in `docs/NOXFUNDS.md`. */
const RULE_CHECKS = [
  "position size",
  "total exposure",
  "permitted market",
  "drawdown",
  "minimum hold",
  "stop-loss present",
] as const;

const AUTOPLAY_MS = 5200;

function usePrefersReducedMotion() {
  const [reduced, setReduced] = useState(false);
  useEffect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const sync = () => setReduced(mq.matches);
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);
  return reduced;
}

/** A node in the diagram. Dim until its step lights it. */
function Node({
  x,
  y,
  w,
  h,
  on,
  tone = "brand",
  children,
}: {
  x: number;
  y: number;
  w: number;
  h: number;
  on: boolean;
  tone?: "brand" | "long" | "short" | "ink";
  children: React.ReactNode;
}) {
  const stroke = on ? `var(--nox-${tone})` : "var(--sf-border)";
  return (
    <g opacity={on ? 1 : 0.42} style={{ transition: "opacity 420ms ease" }}>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        fill="var(--sf-surface)"
        stroke={stroke}
        strokeWidth={on ? 1.6 : 1}
        style={{ transition: "stroke 420ms ease, stroke-width 420ms ease" }}
      />
      {children}
    </g>
  );
}

/** A wire. Draws itself in when its step becomes active. */
function Edge({
  d,
  on,
  tone = "brand",
  dashed = false,
}: {
  d: string;
  on: boolean;
  tone?: "brand" | "long" | "short" | "ink";
  dashed?: boolean;
}) {
  return (
    <path
      d={d}
      fill="none"
      stroke={on ? `var(--nox-${tone})` : "var(--sf-border)"}
      strokeWidth={on ? 1.6 : 1}
      strokeDasharray={dashed ? "4 4" : undefined}
      opacity={on ? 1 : 0.35}
      style={{ transition: "stroke 420ms ease, opacity 420ms ease" }}
    />
  );
}

/**
 * A packet in flight along an edge.
 *
 * Keyed by step in the parent so React remounts it and the CSS animation restarts from 0%
 * rather than continuing mid-flight from the previous step.
 */
function Packet({
  d,
  tone,
  label,
  delay = 0,
  still,
}: {
  d: string;
  tone: "brand" | "long" | "short";
  label?: string;
  delay?: number;
  still: boolean;
}) {
  // A still packet sits at the end of the wire rather than animating along it.
  const style: React.CSSProperties = still
    ? {
        offsetPath: `path("${d}")`,
        offsetDistance: "100%",
        offsetRotate: "0deg",
      }
    : {
        offsetPath: `path("${d}")`,
        offsetRotate: "0deg",
        animation: `nox-travel 2.3s ${delay}s cubic-bezier(0.45, 0, 0.35, 1) infinite`,
      };

  return (
    <g style={style}>
      <circle r={13} fill={`var(--nox-${tone})`} opacity={0.18} />
      <circle r={5.5} fill={`var(--nox-${tone})`} />
      {label ? (
        <text
          y={-18}
          textAnchor="middle"
          fontSize={11}
          fill={`var(--nox-${tone})`}
          className="tnum"
        >
          {label}
        </text>
      ) : null}
    </g>
  );
}

function Label({
  x,
  y,
  children,
  size = 13,
  dim = false,
  bold = false,
  mono = false,
  anchor = "start",
}: {
  x: number;
  y: number;
  children: React.ReactNode;
  size?: number;
  dim?: boolean;
  bold?: boolean;
  mono?: boolean;
  anchor?: "start" | "middle" | "end";
}) {
  return (
    <text
      x={x}
      y={y}
      fontSize={size}
      textAnchor={anchor}
      fill={dim ? "var(--sf-text-dim)" : "var(--sf-text)"}
      fontWeight={bold ? 700 : 400}
      className={mono ? "tnum" : undefined}
    >
      {children}
    </text>
  );
}

export function NoxFlow() {
  const [active, setActive] = useState(0);
  const [paused, setPaused] = useState(false);
  const reduced = usePrefersReducedMotion();
  const still = reduced;
  const timer = useRef<number | undefined>(undefined);

  const step = STEPS[active] as Step;
  const is = (id: StepId) => step.id === id;

  useEffect(() => {
    if (paused || reduced) return;
    timer.current = window.setTimeout(
      () => setActive((i) => (i + 1) % STEPS.length),
      AUTOPLAY_MS
    );
    return () => window.clearTimeout(timer.current);
  }, [active, paused, reduced]);

  return (
    <div
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      onFocusCapture={() => setPaused(true)}
      onBlurCapture={() => setPaused(false)}
    >
      {/* Step chips */}
      <div className="mb-6 flex flex-wrap gap-2">
        {STEPS.map((s, i) => {
          const on = i === active;
          return (
            <button
              key={s.id}
              onClick={() => setActive(i)}
              aria-current={on ? "step" : undefined}
              className={[
                "flex items-center gap-2 border px-3 py-1.5 text-xs uppercase tracking-wider transition-colors",
                on
                  ? "border-[var(--nox-brand)] bg-[color-mix(in_srgb,var(--nox-brand)_12%,transparent)] text-[var(--nox-brand)]"
                  : "border-line text-ink-dim hover:border-[var(--nox-brand)] hover:text-ink",
              ].join(" ")}
            >
              <span className="tnum opacity-70">{s.n}</span>
              {s.chip}
            </button>
          );
        })}
      </div>

      <div className="grid gap-8 lg:grid-cols-[1fr_320px]">
        {/* ---------------------------------------------------------------- diagram */}
        <div className="panel overflow-hidden p-2">
          <svg
            viewBox="0 0 920 470"
            className="h-auto w-full"
            role="img"
            aria-label={`Step ${step.n} of ${STEPS.length}: ${step.title}`}
          >
            {/* wires first, so nodes paint over their ends */}
            <Edge d={EDGE.fund} on={is("fund")} />
            <Edge d={EDGE.capital} on={is("fund") || is("check")} dashed />
            <Edge d={EDGE.order} on={is("order") || is("check")} />
            <Edge d={EDGE.pass} on={is("check")} tone="long" />
            <Edge d={EDGE.reject} on={is("check")} tone="short" dashed />
            <Edge d={EDGE.mark} on={is("mark")} />
            <Edge d={EDGE.settle} on={is("settle")} />

            {/* ---- investor */}
            <Node x={26} y={150} w={150} h={66} on={is("fund") || is("settle")}>
              <Label x={42} y={180} bold>
                Investor
              </Label>
              <Label x={42} y={199} size={11} dim>
                brings the capital
              </Label>
            </Node>

            {/* ---- trader */}
            <Node
              x={26}
              y={302}
              w={150}
              h={66}
              on={is("order") || is("check") || is("settle")}
            >
              <Label x={42} y={332} bold>
                Trader
              </Label>
              <Label x={42} y={351} size={11} dim>
                brings the skill
              </Label>
            </Node>

            {/* ---- mandate vault */}
            <Node
              x={250}
              y={40}
              w={212}
              h={98}
              on={is("fund") || is("mark") || is("settle")}
            >
              <Label x={268} y={68} bold>
                Mandate vault
              </Label>
              <Label x={268} y={88} size={11} dim>
                program address · no private key
              </Label>
              <Label x={268} y={116} size={17} mono>
                200.000000
              </Label>
              <Label x={370} y={116} size={11} dim>
                USDC
              </Label>
            </Node>

            {/* ---- the rulebook */}
            <Node
              x={250}
              y={214}
              w={212}
              h={204}
              on={is("order") || is("check")}
            >
              <Label x={268} y={242} bold>
                NOXFUNDS
              </Label>
              <Label x={268} y={260} size={11} dim>
                checked before the fill
              </Label>
              {RULE_CHECKS.map((r, i) => (
                <g key={r}>
                  <rect
                    x={268}
                    y={276 + i * 22}
                    width={7}
                    height={7}
                    fill={is("check") ? "var(--nox-long)" : "var(--sf-border)"}
                    style={{ transition: "fill 420ms ease" }}
                  />
                  <Label
                    x={285}
                    y={283 + i * 22}
                    size={11.5}
                    dim={!is("check")}
                  >
                    {r}
                  </Label>
                </g>
              ))}
            </Node>

            {/* ---- the venue */}
            <Node
              x={558}
              y={230}
              w={192}
              h={120}
              on={is("check") || is("mark")}
            >
              <Label x={576} y={258} bold>
                SolFX
              </Label>
              <Label x={576} y={276} size={11} dim>
                the exchange underneath
              </Label>
              <Label x={576} y={306} size={11.5} dim>
                position opened
              </Label>
              <Label x={576} y={326} size={11.5} dim>
                stop-loss placed
              </Label>
            </Node>

            {/* ---- payouts */}
            <Node x={558} y={40} w={334} h={140} on={is("settle")}>
              <Label x={576} y={66} bold>
                Settlement
              </Label>
              {[
                ["Investor", "principal + 30% of net"],
                ["Trader", "70% of net"],
                ["Protocol", "5% of gross"],
              ].map(([who, what], i) => (
                <g key={who}>
                  <Label x={576} y={98 + i * 26} size={12}>
                    {who}
                  </Label>
                  <Label x={876} y={98 + i * 26} size={11.5} dim anchor="end">
                    {what}
                  </Label>
                </g>
              ))}
            </Node>

            {/* ---- the reject marker, only meaningful during the check step */}
            <g
              opacity={is("check") ? 1 : 0}
              style={{ transition: "opacity 420ms ease" }}
            >
              <Label x={330} y={452} size={11.5} anchor="middle">
                <tspan fill="var(--nox-short)">breaks a rule → </tspan>
                <tspan fill="var(--sf-text-dim)">
                  transaction reverts, nothing happened
                </tspan>
              </Label>
              <Label x={655} y={206} size={11.5} anchor="middle">
                <tspan fill="var(--nox-long)">passes → forwarded</tspan>
              </Label>
            </g>

            {/* ---- packets. keyed by step so each remount restarts the animation. */}
            <g key={step.id}>
              {is("fund") ? (
                <Packet
                  d={EDGE.fund}
                  tone="brand"
                  label="200 USDC"
                  still={still}
                />
              ) : null}
              {is("order") ? (
                <Packet
                  d={EDGE.order}
                  tone="brand"
                  label="order"
                  still={still}
                />
              ) : null}
              {is("check") ? (
                <>
                  <Packet d={EDGE.pass} tone="long" still={still} />
                  <Packet
                    d={EDGE.reject}
                    tone="short"
                    delay={0.5}
                    still={still}
                  />
                </>
              ) : null}
              {is("mark") ? (
                <Packet
                  d={EDGE.mark}
                  tone="brand"
                  label="equity"
                  still={still}
                />
              ) : null}
              {is("settle") ? (
                <Packet d={EDGE.settle} tone="brand" still={still} />
              ) : null}
            </g>
          </svg>
        </div>

        {/* ---------------------------------------------------------------- caption */}
        <div className="flex flex-col">
          <div className="text-[10px] uppercase tracking-[0.2em] text-[var(--nox-brand)]">
            Step {step.n} of {STEPS.length}
          </div>
          <h3 className="mt-2 text-xl font-bold leading-snug">{step.title}</h3>
          <p className="mt-4 text-sm leading-relaxed text-ink-muted">
            {step.body}
          </p>

          <div className="mt-6 border-l-2 border-[var(--nox-brand)] pl-4">
            <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
              Why it matters
            </div>
            <p className="mt-1.5 text-sm leading-relaxed text-ink">
              {step.point}
            </p>
          </div>

          <div className="mt-auto pt-8">
            <div className="flex gap-1.5" aria-hidden>
              {STEPS.map((s, i) => (
                <span
                  key={s.id}
                  className="h-0.5 flex-1"
                  style={{
                    background:
                      i === active
                        ? "var(--nox-brand)"
                        : i < active
                          ? "color-mix(in srgb, var(--nox-brand) 35%, transparent)"
                          : "var(--sf-border)",
                    transition: "background 420ms ease",
                  }}
                />
              ))}
            </div>
            <p className="mt-3 text-[11px] text-ink-dim">
              {reduced
                ? "Motion reduced at your request. Use the steps above."
                : paused
                  ? "Paused. Move the pointer away to resume."
                  : "Advances on its own. Hover to pause, or pick a step."}
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}
