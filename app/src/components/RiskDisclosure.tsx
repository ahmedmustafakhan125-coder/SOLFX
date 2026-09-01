import { useEffect, useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";

const KEY = "solfx.risk-acknowledged.v1";

/**
 * Shown once, on the first wallet connection.
 *
 * §10.2 calls this required both ethically and for any future licensing conversation, and the
 * list below is not boilerplate: every item is a thing this protocol will actually do to a
 * position, described in the terms the code uses.
 *
 * Acknowledgement lives in `localStorage`, which means per browser and per device. That is the
 * honest scope for it — it records that this person was shown the warning here, and nothing
 * more. Putting it on chain would cost a transaction and still prove no more than that a key
 * signed something.
 */
export function RiskDisclosure() {
  const { wallet } = useWalletConnection();
  const [acknowledged, setAcknowledged] = useState(true);

  useEffect(() => {
    if (!wallet) return;
    try {
      setAcknowledged(localStorage.getItem(KEY) === "1");
    } catch {
      // Private browsing, or site data blocked. Show it rather than assume consent.
      setAcknowledged(false);
    }
  }, [wallet]);

  if (!wallet || acknowledged) return null;

  function accept() {
    try {
      localStorage.setItem(KEY, "1");
    } catch {
      // Nothing to do — it will be shown again next time, which is the safe direction.
    }
    setAcknowledged(true);
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4">
      <div className="max-h-[85vh] w-full max-w-lg overflow-y-auto border border-[var(--sf-border)] bg-surface p-6">
        <h2 className="text-sm font-semibold uppercase tracking-[0.16em]">
          Before you trade
        </h2>
        <p className="mt-3 text-xs leading-relaxed text-ink-dim">
          SolFX is leveraged, non-custodial and unaudited. Read this once.
        </p>

        <ul className="mt-4 space-y-3 text-xs leading-relaxed text-ink-dim">
          <li>
            <span className="text-ink">Leverage magnifies losses.</span> A small move against
            you can remove your entire margin. Maximum leverage is set per market, not
            protocol-wide.
          </li>
          <li>
            <span className="text-ink">You can be liquidated.</span> When equity falls below
            the maintenance margin, anyone may close your position and collect a fee from it.
            This is automatic and does not wait for you.
          </li>
          <li>
            <span className="text-ink">A stop is not a guaranteed price.</span> Triggers fire
            on the oracle and then fill at the execution price, including the spread. In a gap
            the fill may be far past your trigger.
          </li>
          <li>
            <span className="text-ink">Sessions and gaps.</span> FX and metals do not trade
            continuously. A market can reopen far from where it closed, and the protocol
            correctly refuses to trade while its oracle is stale — which can also mean you
            cannot close when you want to.
          </li>
          <li>
            <span className="text-ink">Auto-deleveraging.</span> In extreme conditions
            profitable positions can be reduced to keep the pool solvent.
          </li>
          <li>
            <span className="text-ink">Smart-contract risk.</span> SolFX has had{" "}
            <span className="text-ink">no external audit</span>. A bug can lose funds, and
            nobody can reverse a Solana transaction.
          </li>
        </ul>

        <button
          onClick={accept}
          className="mt-6 w-full bg-brand py-2.5 text-xs font-semibold text-white hover:bg-brand-dim"
        >
          I understand
        </button>
      </div>
    </div>
  );
}
