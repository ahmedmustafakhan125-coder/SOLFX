import { useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";

import { buildDeposit, buildInitUserAccount, buildWithdraw } from "@/lib/account";
import { useAccount } from "@/hooks/useAccount";
import { useSigner } from "@/hooks/useSigner";
import { useSend } from "@/hooks/useSend";
import { fmtUsd } from "@/lib/format";

const QUOTE_PRECISION = 1_000_000n;

/** Whole USDC to base units. Rejects anything that is not a plain decimal. */
function toQuote(text: string): bigint | undefined {
  const t = text.trim();
  if (!/^\d+(\.\d{0,6})?$/.test(t)) return undefined;
  const [whole = "0", frac = ""] = t.split(".");
  const v = BigInt(whole) * QUOTE_PRECISION + BigInt(frac.padEnd(6, "0") || "0");
  return v > 0n ? v : undefined;
}

export function AccountPanel() {
  const { wallet } = useWalletConnection();
  const signer = useSigner();
  const { status, loading, error, refresh } = useAccount();
  const { send, busy, signature, error: sendError, logs, reset } = useSend();
  const [amount, setAmount] = useState("1000");
  const [mode, setMode] = useState<"deposit" | "withdraw">("deposit");

  if (!wallet) {
    return (
      <div className="border-b border-line-soft p-4 text-xs text-ink-dim">
        Connect a wallet to fund an account and trade.
      </div>
    );
  }

  const parsed = toQuote(amount);
  // What the amount is checked against: the wallet's own balance on the way in, and the
  // protocol's `free_collateral` field on the way out. Both are figures we read rather than
  // compute — the margin rule that decides a withdrawal belongs to the program, and a second
  // copy of it here would be an unaudited margin engine that disagrees by a rounding step.
  const ceiling = mode === "deposit" ? (status?.walletUsdc ?? 0n) : (status?.freeCollateral ?? 0n);

  async function onInit() {
    if (!signer) return;
    reset();
    const ix = await buildInitUserAccount(signer);
    if (await send([ix])) refresh();
  }

  async function onSubmit() {
    if (!signer || !status || parsed === undefined) return;
    reset();
    const ixs =
      mode === "deposit"
        ? await buildDeposit(signer, status.usdcMint, parsed, !status.hasAta)
        : await buildWithdraw(signer, status.usdcMint, parsed);
    if (await send(ixs)) refresh();
  }

  return (
    <div className="space-y-3 border-b border-line-soft p-4">
      <div className="flex items-baseline justify-between">
        <span className="text-[10px] uppercase tracking-[0.18em] text-ink-dim">Account</span>
        {loading ? <span className="text-[10px] text-ink-dim">reading…</span> : null}
      </div>

      {error ? (
        <div className="rounded border border-short/40 bg-short/10 p-2 text-[11px] text-short">
          {error}
        </div>
      ) : null}

      {status ? (
        <>
          <div className="space-y-1.5 rounded-md border border-line-soft bg-surface-high p-3 text-xs">
            <div className="flex items-baseline justify-between">
              <span className="text-ink-dim">Free collateral</span>
              <span className="tnum">${fmtUsd(status.freeCollateral)}</span>
            </div>
            <div className="flex items-baseline justify-between">
              <span className="text-ink-dim">Wallet USDC</span>
              <span className="tnum">${fmtUsd(status.walletUsdc)}</span>
            </div>
          </div>

          {!status.hasUserAccount ? (
            <>
              <p className="text-[11px] leading-relaxed text-ink-dim">
                This wallet has no SolFX account yet. One transaction creates it; the
                referrer is bound once at creation and never reassigned.
              </p>
              <button
                onClick={() => void onInit()}
                disabled={busy || !signer}
                className="w-full rounded-md bg-brand py-2 text-xs font-semibold text-white hover:bg-brand-dim disabled:opacity-50"
              >
                {busy ? "Confirming…" : "Create account"}
              </button>
            </>
          ) : (
            <>
              <div className="flex gap-1">
                {(["deposit", "withdraw"] as const).map((m) => (
                  <button
                    key={m}
                    type="button"
                    onClick={() => setMode(m)}
                    className={
                      "flex-1 rounded-md py-1.5 text-[11px] font-semibold capitalize transition-colors " +
                      (mode === m
                        ? "bg-surface-high text-ink"
                        : "text-ink-dim hover:text-ink")
                    }
                  >
                    {m}
                  </button>
                ))}
              </div>

              <label className="block">
                <span className="mb-1 block text-[10px] uppercase tracking-[0.14em] text-ink-dim">
                  {mode === "deposit" ? "Deposit collateral (USDC)" : "Withdraw collateral (USDC)"}
                </span>
                <input
                  value={amount}
                  onChange={(e) => setAmount(e.target.value)}
                  inputMode="decimal"
                  className="tnum w-full rounded-md border border-line bg-surface px-3 py-2 text-sm outline-none focus:border-brand"
                />
              </label>
              {amount.trim() !== "" && parsed === undefined ? (
                <p className="text-[11px] text-warn">
                  Enter a positive amount with at most six decimals.
                </p>
              ) : null}
              {parsed !== undefined && parsed > ceiling ? (
                <p className="text-[11px] text-warn">
                  {mode === "deposit"
                    ? "More than this wallet holds. The token transfer would fail."
                    : "More than your free collateral. The program would refuse it."}
                </p>
              ) : null}
              {mode === "withdraw" ? (
                <p className="text-[11px] leading-relaxed text-ink-dim">
                  Free collateral is what is not backing an open position. The program checks
                  this again on chain and refuses anything that would leave a position
                  under-margined — this figure is its own, not an estimate.
                </p>
              ) : null}
              <button
                onClick={() => void onSubmit()}
                disabled={busy || !signer || parsed === undefined || parsed > ceiling}
                className="w-full rounded-md bg-brand py-2 text-xs font-semibold text-white hover:bg-brand-dim disabled:opacity-50"
              >
                {busy
                  ? "Confirming…"
                  : mode === "withdraw"
                    ? "Withdraw"
                    : status.hasAta
                      ? "Deposit"
                      : "Create token account & deposit"}
              </button>
            </>
          )}
        </>
      ) : null}

      {signature ? (
        <a
          href={`https://explorer.solana.com/tx/${signature}?cluster=devnet`}
          target="_blank"
          rel="noreferrer"
          className="block truncate rounded border border-long/30 bg-long/10 p-2 text-[11px] text-long underline"
        >
          confirmed · {signature.slice(0, 20)}…
        </a>
      ) : null}

      {sendError ? (
        <div className="space-y-1 rounded border border-short/40 bg-short/10 p-2">
          <div className="text-[11px] text-short">{sendError}</div>
          {logs?.length ? (
            <pre className="tnum max-h-40 overflow-auto whitespace-pre-wrap text-[10px] leading-snug text-ink-muted">
              {logs.join("\n")}
            </pre>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
