import { useState } from "react";
import { useWalletConnection } from "@solana/react-hooks";
import {
  navPerShare,
  previewDeposit,
  previewWithdrawal,
  secondsUntilUnlock,
} from "@solfx/client";

import { Header } from "@/components/Header";
import { usePool } from "@/hooks/usePool";
import { useSend } from "@/hooks/useSend";
import { useSigner } from "@/hooks/useSigner";
import { RPC_URL, rpcLabel } from "@/config";
import {
  buildAddLiquidity,
  buildCancelWithdrawal,
  buildRemoveLiquidity,
  buildRequestWithdrawal,
} from "@/lib/pool";
import { fmtBps, fmtUsd } from "@/lib/format";

const QUOTE_PRECISION = 1_000_000n;

/**
 * Slippage tolerance on the LP side, in bps.
 *
 * NAV per share moves with every trade the pool is counterparty to, so the shares a deposit
 * mints — and the USDC a redemption returns — can differ between the page reading the pool
 * and the transaction landing. `add_liquidity` and `remove_liquidity` both compare against a
 * bound, and there is no value that disables the comparison. 50 bps is wide enough that
 * ordinary drift does not bounce a deposit and tight enough to catch a pool that moved.
 */
const SLIPPAGE_BPS = 50n;
const BPS = 10_000n;

/** Whole USDC to base units. Rejects anything that is not a plain decimal. */
function toQuote(text: string): bigint | undefined {
  const t = text.trim();
  if (!/^\d+(\.\d{0,6})?$/.test(t)) return undefined;
  const [whole = "0", frac = ""] = t.split(".");
  const v =
    BigInt(whole) * QUOTE_PRECISION + BigInt(frac.padEnd(6, "0") || "0");
  return v > 0n ? v : undefined;
}

function fmtDuration(seconds: bigint): string {
  if (seconds <= 0n) return "ready";
  const h = seconds / 3600n;
  const m = (seconds % 3600n) / 60n;
  const s = seconds % 60n;
  if (h > 0n) return `${h}h ${m}m`;
  if (m > 0n) return `${m}m ${s}s`;
  return `${s}s`;
}

function Row({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-1.5">
      <span className="text-ink-dim">
        {label}
        {hint ? (
          <span className="ml-1 text-[10px] text-ink-muted">{hint}</span>
        ) : null}
      </span>
      <span className="tnum text-ink">{value}</span>
    </div>
  );
}

/**
 * The other side of the book.
 *
 * SolFX is a B-book: the pool is the counterparty to every trade, so an LP here is not
 * lending to traders, they *are* the house. The page says so rather than describing this as
 * yield, because a provider who does not understand that they absorb trader profits is a
 * provider who will be surprised exactly once.
 */
export function Pool() {
  const { wallet } = useWalletConnection();
  const signer = useSigner();
  const { status, loading, error, refresh } = usePool();
  const { send, busy, signature, error: sendError, logs, reset } = useSend();
  const [amount, setAmount] = useState("1000");
  const [shareText, setShareText] = useState("");

  const nav = status
    ? navPerShare(status.pool.aum, status.pool.lpTokenSupply)
    : QUOTE_PRECISION;
  const myValue = status
    ? (status.shares * status.pool.aum) /
      (status.pool.lpTokenSupply === 0n ? 1n : status.pool.lpTokenSupply)
    : 0n;
  const myShareBps =
    status && status.pool.lpTokenSupply > 0n
      ? Number((status.shares * 10_000n) / status.pool.lpTokenSupply)
      : 0;

  const depositAmount = toQuote(amount);
  const depositPreview =
    status && depositAmount !== undefined
      ? previewDeposit(depositAmount, status.pool)
      : undefined;
  const minLpOut = depositPreview
    ? (depositPreview.shares * (BPS - SLIPPAGE_BPS)) / BPS
    : 0n;

  const requestShares = toQuote(shareText);
  const pending = status?.pendingWithdrawal ?? null;
  const untilUnlock =
    pending && status
      ? secondsUntilUnlock(pending.unlockAt, status.nowSeconds)
      : 0n;
  const withdrawalPreview =
    pending && status
      ? previewWithdrawal(pending.shares, status.pool)
      : undefined;
  const minUsdcOut = withdrawalPreview
    ? (withdrawalPreview.payout * (BPS - SLIPPAGE_BPS)) / BPS
    : 0n;

  async function onDeposit() {
    if (!signer || !status || depositAmount === undefined) return;
    reset();
    const ixs = await buildAddLiquidity(
      signer,
      status,
      depositAmount,
      minLpOut
    );
    if (await send(ixs)) refresh();
  }

  async function onRequest() {
    if (!signer || !status || requestShares === undefined) return;
    reset();
    const ixs = await buildRequestWithdrawal(
      signer,
      status.lpMint,
      requestShares
    );
    if (await send(ixs)) refresh();
  }

  async function onWithdraw() {
    if (!signer || !status) return;
    reset();
    const ixs = await buildRemoveLiquidity(signer, status, minUsdcOut);
    if (await send(ixs)) refresh();
  }

  async function onCancel() {
    if (!signer) return;
    reset();
    const ixs = await buildCancelWithdrawal(signer);
    if (await send(ixs)) refresh();
  }

  return (
    <div className="min-h-screen bg-surface text-ink">
      <Header rpcLabel={rpcLabel(RPC_URL)} />

      <main className="mx-auto max-w-5xl px-5 py-8">
        <div className="mb-6">
          <h1 className="text-2xl font-bold tracking-tight">Liquidity pool</h1>
          <p className="mt-1 max-w-2xl text-sm leading-relaxed text-ink-dim">
            SolFX is a B-book venue: this pool is the counterparty to every
            trade. It earns the spread, the fees and the losses of unprofitable
            traders — and it pays the profits of profitable ones. That is the
            position you are taking, not a yield.
          </p>
        </div>

        {error ? (
          <div className="mb-6 rounded border border-short/40 bg-short/10 p-3 text-sm text-short">
            Could not read the pool: {error}
          </div>
        ) : null}

        {loading && !status ? (
          <div className="text-sm text-ink-dim">reading the pool…</div>
        ) : null}

        {status ? (
          <div className="grid gap-6 md:grid-cols-2">
            {/* --- the pool itself, public --- */}
            <section className="rounded-lg border border-line-soft bg-surface-high p-4">
              <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
                The pool
              </h2>
              <div className="divide-y divide-line-soft text-xs">
                <Row
                  label="Assets under management"
                  value={`$${fmtUsd(status.pool.aum)}`}
                />
                <Row
                  label="NAV per share"
                  value={`$${fmtUsd(nav, 6)}`}
                  hint="slpUSD"
                />
                <Row
                  label="Shares outstanding"
                  value={fmtUsd(status.pool.lpTokenSupply)}
                />
                <Row
                  label="Queued to leave"
                  value={fmtUsd(status.facts.pendingWithdrawalShares)}
                  hint="shares"
                />
                <Row
                  label="High-water mark"
                  value={`$${fmtUsd(status.pool.highWaterMarkPerShare, 6)}`}
                />
                <Row
                  label="Withdrawal cooldown"
                  value={fmtDuration(
                    BigInt(status.facts.withdrawalCooldownSeconds)
                  )}
                />
                <Row
                  label="Exit fee"
                  value={fmtBps(status.pool.exitFeeBps)}
                  hint="stays in the pool"
                />
                <Row
                  label="Performance fee"
                  value={fmtBps(status.pool.performanceFeeBps)}
                  hint="above the mark only"
                />
                <Row
                  label="Lifetime deposited"
                  value={`$${fmtUsd(status.facts.totalDeposited)}`}
                />
                <Row
                  label="Lifetime withdrawn"
                  value={`$${fmtUsd(status.facts.totalWithdrawn)}`}
                />
              </div>
              <p className="mt-3 text-[11px] leading-relaxed text-ink-muted">
                The exit fee is not revenue. It is paid to the providers who
                stayed, because they are the ones a fast in-and-out would have
                diluted.
              </p>
            </section>

            {/* --- the connected wallet's position --- */}
            <section className="rounded-lg border border-line-soft bg-surface-high p-4">
              <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-ink-dim">
                Your position
              </h2>

              {!wallet ? (
                <p className="text-xs text-ink-dim">
                  Connect a wallet to provide liquidity. Everything above is
                  public and true whether you connect or not.
                </p>
              ) : (
                <>
                  <div className="divide-y divide-line-soft text-xs">
                    <Row
                      label="Shares held"
                      value={fmtUsd(status.shares)}
                      hint="slpUSD"
                    />
                    <Row label="Value at NAV" value={`$${fmtUsd(myValue)}`} />
                    <Row label="Share of pool" value={fmtBps(myShareBps)} />
                    <Row
                      label="Wallet USDC"
                      value={`$${fmtUsd(status.walletUsdc)}`}
                    />
                  </div>

                  {/* --- deposit --- */}
                  <div className="mt-4 space-y-2 border-t border-line-soft pt-4">
                    <label className="block">
                      <span className="mb-1 block text-[10px] uppercase tracking-[0.14em] text-ink-dim">
                        Provide liquidity (USDC)
                      </span>
                      <input
                        value={amount}
                        onChange={(e) => setAmount(e.target.value)}
                        inputMode="decimal"
                        className="tnum w-full rounded-md border border-line bg-surface px-3 py-2 text-sm outline-none focus:border-brand"
                      />
                    </label>
                    {amount.trim() !== "" && depositAmount === undefined ? (
                      <p className="text-[11px] text-warn">
                        Enter a positive amount with at most six decimals.
                      </p>
                    ) : null}
                    {depositAmount !== undefined &&
                    depositAmount > status.walletUsdc ? (
                      <p className="text-[11px] text-warn">
                        More than this wallet holds. The token transfer would
                        fail.
                      </p>
                    ) : null}
                    {depositPreview ? (
                      <div className="tnum rounded border border-line-soft bg-surface p-2 text-[11px] text-ink-dim">
                        <div className="flex justify-between">
                          <span>Shares minted</span>
                          <span className="text-ink">
                            {fmtUsd(depositPreview.shares)}
                          </span>
                        </div>
                        <div className="flex justify-between">
                          <span>Minimum accepted</span>
                          <span className="text-ink">{fmtUsd(minLpOut)}</span>
                        </div>
                      </div>
                    ) : null}
                    <button
                      onClick={() => void onDeposit()}
                      disabled={
                        busy ||
                        !signer ||
                        depositAmount === undefined ||
                        depositAmount > status.walletUsdc
                      }
                      className="w-full rounded-md bg-brand py-2 text-xs font-semibold text-white hover:bg-brand-dim disabled:opacity-50"
                    >
                      {busy
                        ? "Confirming…"
                        : status.hasLpAccount
                          ? "Provide liquidity"
                          : "Create share account & provide"}
                    </button>
                  </div>

                  {/* --- withdraw, which is a state machine and not a button --- */}
                  <div className="mt-4 space-y-2 border-t border-line-soft pt-4">
                    <span className="block text-[10px] uppercase tracking-[0.14em] text-ink-dim">
                      Withdraw
                    </span>

                    {!pending ? (
                      <>
                        <p className="text-[11px] leading-relaxed text-ink-muted">
                          Withdrawing takes two transactions with the cooldown
                          between them. The delay is what stops a provider
                          depositing ahead of a known trader loss and leaving
                          ahead of the gain.
                        </p>
                        <input
                          value={shareText}
                          onChange={(e) => setShareText(e.target.value)}
                          inputMode="decimal"
                          placeholder="shares to withdraw"
                          className="tnum w-full rounded-md border border-line bg-surface px-3 py-2 text-sm outline-none focus:border-brand"
                        />
                        {requestShares !== undefined &&
                        requestShares > status.shares ? (
                          <p className="text-[11px] text-warn">
                            More shares than you hold.
                          </p>
                        ) : null}
                        <button
                          onClick={() => void onRequest()}
                          disabled={
                            busy ||
                            !signer ||
                            requestShares === undefined ||
                            requestShares > status.shares
                          }
                          className="w-full rounded-md border border-line bg-surface-highest py-2 text-xs font-semibold text-ink hover:border-brand disabled:opacity-50"
                        >
                          Start withdrawal
                        </button>
                      </>
                    ) : (
                      <>
                        <div className="tnum rounded border border-line-soft bg-surface p-2 text-[11px] text-ink-dim">
                          <div className="flex justify-between">
                            <span>Shares queued</span>
                            <span className="text-ink">
                              {fmtUsd(pending.shares)}
                            </span>
                          </div>
                          <div className="flex justify-between">
                            <span>Value before fees</span>
                            <span className="text-ink">
                              ${fmtUsd(withdrawalPreview?.gross ?? 0n)}
                            </span>
                          </div>
                          <div className="flex justify-between">
                            <span>Performance fee</span>
                            <span className="text-ink">
                              −$
                              {fmtUsd(withdrawalPreview?.performanceFee ?? 0n)}
                            </span>
                          </div>
                          <div className="flex justify-between">
                            <span>
                              Exit fee
                              {withdrawalPreview?.drainsThePool
                                ? " (waived — empties the pool)"
                                : ""}
                            </span>
                            <span className="text-ink">
                              −${fmtUsd(withdrawalPreview?.exitFee ?? 0n)}
                            </span>
                          </div>
                          <div className="mt-1 flex justify-between border-t border-line-soft pt-1">
                            <span>You receive</span>
                            <span className="text-ink">
                              ${fmtUsd(withdrawalPreview?.payout ?? 0n)}
                            </span>
                          </div>
                        </div>

                        {untilUnlock > 0n ? (
                          <p className="text-[11px] text-warn">
                            Unlocks in {fmtDuration(untilUnlock)}. The program
                            measures this against its own clock, so the moment
                            shown is the chain's.
                          </p>
                        ) : (
                          <p className="text-[11px] text-long">
                            Ready to settle.
                          </p>
                        )}

                        <div className="flex gap-2">
                          <button
                            onClick={() => void onWithdraw()}
                            disabled={busy || !signer || untilUnlock > 0n}
                            className="flex-1 rounded-md bg-brand py-2 text-xs font-semibold text-white hover:bg-brand-dim disabled:opacity-50"
                          >
                            {busy ? "Confirming…" : "Withdraw"}
                          </button>
                          <button
                            onClick={() => void onCancel()}
                            disabled={busy || !signer}
                            className="rounded-md border border-line bg-surface-highest px-3 py-2 text-xs font-semibold text-ink-dim hover:text-ink disabled:opacity-50"
                          >
                            Cancel
                          </button>
                        </div>
                        <p className="text-[11px] leading-relaxed text-ink-muted">
                          The preview is computed with the engine's own
                          arithmetic, but the pool can move before this lands.
                          It will not pay less than ${fmtUsd(minUsdcOut)}.
                        </p>
                      </>
                    )}
                  </div>
                </>
              )}

              {signature ? (
                <a
                  href={`https://explorer.solana.com/tx/${signature}?cluster=devnet`}
                  target="_blank"
                  rel="noreferrer"
                  className="mt-3 block truncate rounded border border-long/30 bg-long/10 p-2 text-[11px] text-long underline"
                >
                  confirmed · {signature.slice(0, 20)}…
                </a>
              ) : null}

              {sendError ? (
                <div className="mt-3 space-y-1 rounded border border-short/40 bg-short/10 p-2">
                  <div className="text-[11px] text-short">{sendError}</div>
                  {logs?.length ? (
                    <pre className="tnum max-h-40 overflow-auto whitespace-pre-wrap text-[10px] leading-snug text-ink-muted">
                      {logs.join("\n")}
                    </pre>
                  ) : null}
                </div>
              ) : null}
            </section>
          </div>
        ) : null}
      </main>
    </div>
  );
}
