import { Link } from "react-router-dom";

/**
 * Everything personal lives in this one object so it can be edited without touching markup.
 *
 * Only `name` and `email` are filled: they come from the repository's own git identity and
 * are therefore verifiable. The rest is left blank on purpose — a bio invented on someone's
 * behalf is worse than an empty field, and this page carries a real person's name.
 */
const ME = {
  name: "Ahmed Mustafa Khan",
  email: "ahmedmustafakhan125@gmail.com",
  // --- fill these in ---
  role: "", // e.g. "Computer science student · building SolFX solo"
  location: "", // e.g. "Karachi, Pakistan"
  bio: [
    // One paragraph per entry. Left empty deliberately — see the note above.
  ] as string[],
  links: [
    // { label: "GitHub", href: "https://github.com/..." },
    // { label: "X", href: "https://x.com/..." },
    // { label: "LinkedIn", href: "https://linkedin.com/in/..." },
  ] as { label: string; href: string }[],
};

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="border-b border-line-soft py-3 last:border-0">
      <div className="text-[10px] uppercase tracking-[0.16em] text-ink-dim">
        {label}
      </div>
      <div className="mt-1 text-sm text-ink">{children}</div>
    </div>
  );
}

export function About() {
  const incomplete = ME.role === "" || ME.bio.length === 0;

  return (
    <div className="min-h-screen bg-bg text-ink">
      <section className="border-b border-line-soft px-6 py-20 md:px-10">
        <div className="mx-auto max-w-4xl">
          <div className="text-[10px] uppercase tracking-[0.2em] text-brand">
            About
          </div>
          <h1 className="mt-2 text-3xl font-extrabold tracking-tight md:text-4xl">
            Who built this
          </h1>
        </div>
      </section>

      <section className="px-6 py-14 md:px-10">
        <div className="mx-auto grid max-w-4xl gap-10 md:grid-cols-[1fr_260px]">
          <div>
            <h2 className="text-xl font-bold">{ME.name}</h2>
            {ME.role ? (
              <p className="mt-1 text-sm text-brand-soft">{ME.role}</p>
            ) : null}

            {ME.bio.length > 0 ? (
              <div className="mt-6 space-y-4 text-sm leading-relaxed text-ink-muted">
                {ME.bio.map((p) => (
                  <p key={p.slice(0, 24)}>{p}</p>
                ))}
              </div>
            ) : (
              <div className="mt-6 rounded-lg border border-warn/25 bg-warn/5 p-4 text-xs leading-relaxed text-warn">
                This section is intentionally empty. Fill in{" "}
                <span className="tnum">ME</span> at the top of{" "}
                <span className="tnum">src/pages/About.tsx</span> — role,
                location, bio paragraphs and links. Nothing here was written on
                your behalf.
              </div>
            )}

            <h3 className="mt-12 text-sm font-semibold">What SolFX is</h3>
            <div className="mt-3 space-y-4 text-sm leading-relaxed text-ink-muted">
              <p>
                A non-custodial forex brokerage on Solana. Margin FX, metals and
                crypto, settled in USDC, priced by Pyth, with the trading rules
                enforced by an on-chain program rather than by a broker's back
                office.
              </p>
              <p>
                It exists because retail FX is the one large market that
                on-chain trading has mostly skipped. Perpetual DEXs compete over
                crypto; forex is still served by brokers whose spreads, swap
                rates and execution you cannot inspect. Here the swap rate is a
                field you can read before you open, and the rules that would
                reject your order are the same ones anyone else can read.
              </p>
              <p>
                Built from the arithmetic up: a dependency-free maths crate with
                property tests, then the vault, then positions, risk,
                liquidations, an LP vault and an introducing-broker programme —
                each with exit criteria rather than a deadline.
              </p>
            </div>

            <h3 className="mt-12 text-sm font-semibold">
              Where it honestly stands
            </h3>
            <ul className="mt-3 space-y-2 text-sm leading-relaxed text-ink-muted">
              <li>
                · Devnet only. Test money. Not production, and not investment
                advice.
              </li>
              <li>
                ·{" "}
                <strong className="text-ink">
                  No external audit has been done.
                </strong>
              </li>
              <li>
                · Six markets, limited by oracle access rather than by the
                engine.
              </li>
              <li>
                · 549 tests pass. That means the behaviours someone thought to
                test behave — nothing more.
              </li>
            </ul>
          </div>

          <aside className="space-y-1 rounded-lg border border-line-soft bg-surface p-5">
            <Field label="Contact">
              <a
                className="underline hover:text-brand-soft"
                href={`mailto:${ME.email}`}
              >
                {ME.email}
              </a>
            </Field>
            {ME.location ? <Field label="Based in">{ME.location}</Field> : null}
            {ME.links.length > 0 ? (
              <Field label="Elsewhere">
                <div className="flex flex-col gap-1">
                  {ME.links.map((l) => (
                    <a
                      key={l.href}
                      href={l.href}
                      target="_blank"
                      rel="noreferrer"
                      className="underline hover:text-brand-soft"
                    >
                      {l.label}
                    </a>
                  ))}
                </div>
              </Field>
            ) : null}
            <Field label="Program">
              <span className="tnum break-all text-xs text-ink-muted">
                2EQzy2Mzixi54tJkMbWWqJFoayUoGBNwZCEJCy44ZVKi
              </span>
            </Field>
          </aside>
        </div>
      </section>

      {incomplete ? null : <div />}

      <footer className="border-t border-line-soft px-6 py-10 md:px-10">
        <div className="mx-auto flex max-w-4xl flex-wrap items-center justify-between gap-4 text-xs text-ink-dim">
          <Link to="/" className="hover:text-ink">
            ← SolFX
          </Link>
          <Link to="/trade" className="hover:text-ink">
            Open the terminal
          </Link>
        </div>
      </footer>
    </div>
  );
}
