# 100 test cases — localnet

**Runnable:** [`scripts/test-100.sh`](../../scripts/test-100.sh). `--list` to see the
numbers, `test-100.sh 26 45` for a range. Last full run: **83 pass, 0 fail, 17 manual.**

The 17 are cases this harness genuinely cannot drive — a fresh wallet, a weekend, an admin
pause instruction the CLI does not expose, an adverse price move. They are listed because
they matter, not because they are automatable.

## First run found four failures, all in the test script

Worth recording, because the shape repeats:

- **26** asserted `LeverageExceeded`. The real error is `LeverageTooHigh` — the assertion was
  guessing at a name it had not checked.
- **36** expected `$1` of notional to be refused. `MIN_NOTIONAL_QUOTE` is exactly `$1.00` and
  the floor is **inclusive**, so the protocol was right to take it. Now asks for `$0.08`.
- **38 and 39** were collateral damage: case 36 succeeded unexpectedly and left a position on
  nonce 0, so both later cases failed on "account already in use" rather than their own
  condition. One bad expectation looked like three bugs.

`expect_fail` now closes any position an unexpectedly-successful case leaves behind.

---

# Reference — what each case is for


Complements the 529 automated tests. Those run against LiteSVM in-process; these run against
a **real validator with live Pyth prices**, which is where a different class of bug lives:
clock skew, oracle staleness, transaction size, account plumbing, and every assumption a
client makes about the program.

**Every failure so far has been in the client, not the protocol.** That is the expected
shape. When a case below fails, suspect the CLI first.

## Before you start

```bash
# terminal 1
./scripts/deploy-localnet.sh
# terminal 2
solana config set --url http://127.0.0.1:8899 && solana airdrop 100
./target/debug/init-protocol
# terminal 3 — leave running, or everything fails on staleness after 60s
./target/debug/price-poster --rpc-url http://127.0.0.1:8899
# terminal 2
./target/debug/trade setup --amount 100000
```

Shorthand below: `trade` = `./target/debug/trade`.

**Session gate.** Only BTC/USD, ETH/USD and SOL/USD are continuous. FX and metals trade
Sunday 21:00 → Friday 21:00 UTC and are correctly refused outside it. Cases marked **[FX]**
need an open session; check with `date -u`.

**Marked [needs tooling]** — cases that cannot be driven from the current CLI. They are
listed because they matter, not because they are runnable today.

---

## 1. Account lifecycle (1–10)

| # | Command | Expect |
|---|---|---|
| 1 | `trade status` before setup | "no user account" |
| 2 | `trade setup --amount 100000` | two signatures, account created |
| 3 | `trade status` | free collateral 100000.000000 |
| 4 | `trade setup --amount 1000` again | account exists, deposits again |
| 5 | `trade status` | collateral now 101000.000000 |
| 6 | `trade setup --amount 0` | rejected: zero amount |
| 7 | `trade open` before setup (fresh wallet) | `AccountNotInitialized` |
| 8 | `trade status` shows all 5 markets with leverage caps | 50x / 50x / 10x / 20x / 10x |
| 9 | `trade status` shows LP vault ≥ 1,000,000 | seeded |
| 10 | Delete `deployment.json`, run `trade status` | clear error naming the file |

## 2. Opening — the happy paths (11–25)

| # | Command | Expect |
|---|---|---|
| 11 | `trade open --market BTC/USD --direction long --notional 1000 --collateral 200` | lands |
| 12 | `trade status` | one position, entry **above** oracle (a buy crosses the spread) |
| 13 | Same but `--direction short` | entry **below** oracle |
| 14 | `--notional 100 --collateral 50` | small position lands |
| 15 | `--notional 5000 --collateral 1000` | 5x leverage, lands |
| 16 | `--lots 0.01` on BTC | ≈0.01 BTC ≈ $774 notional |
| 17 | `--lots 1` on BTC | 1 BTC ≈ $77k notional; needs ≥$7.7k margin at 10x |
| 18 | ETH/USD long `--notional 1000` | lands |
| 19 | SOL/USD long `--notional 1000` | lands |
| 20 | Two markets at once (BTC + ETH) | both open, `open_positions` = 2 |
| 21 | `--nonce 0` and `--nonce 1`, same market | both open, distinct PDAs |
| 22 | `--nonce 0` long and `--nonce 1` short | hedged; OI shows on both sides |
| 23 | Open on nonces 0,1,2,3 | four positions on one market |
| 24 | **[FX]** EUR/USD long `--notional 1000 --collateral 100` | lands at 50x cap |
| 25 | **[FX]** USD/INR long `--notional 1000 --collateral 200` | the EM pair the positioning rests on |

## 3. Opening — must be REJECTED (26–45)

A rejection here is the protocol working.

| # | Command | Expect |
|---|---|---|
| 26 | `--notional 100000 --collateral 100` on BTC | `LeverageExceeded` (10x cap) |
| 27 | `--notional 1000000000 --collateral 100` | leverage or OI cap |
| 28 | `--notional 0` | rejected client-side |
| 29 | `--collateral 0` | `ZeroAmount` |
| 30 | `--lots 0` | rejected client-side |
| 31 | `--lots -1` | rejected client-side |
| 32 | `--lots abc` | rejected client-side |
| 33 | `--lots 0.00001` | finer than resolution |
| 34 | `--market GBP/USD` | not listed; lists the 5 that are |
| 35 | `--market NONSENSE` | same |
| 36 | `--notional 1` on BTC | below `$1` notional floor, or dust |
| 37 | Reuse an occupied `--nonce` | account already in use |
| 38 | `--collateral 999999999` (more than you hold) | insufficient collateral |
| 39 | `--slippage-bps 0` | `SlippageExceeded` — the spread alone breaks a zero bound |
| 40 | `--slippage-bps 1` | likely rejected; spread exceeds 1bp |
| 41 | **[FX]** EUR/USD during the weekend | `MarketClosedForOpens` |
| 42 | **[FX]** XAU/USD during the weekend | same |
| 43 | Open when protocol paused | `ProtocolPaused` **[needs tooling]** |
| 44 | Open on a `ReduceOnly` market | `MarketClosedForOpens` **[needs tooling]** |
| 45 | Open on a `Halted` market | same **[needs tooling]** |

## 4. Sizing — the class of bug that reads as plausible (46–55)

| # | Check | Expect |
|---|---|---|
| 46 | `--lots 0.01` BTC vs `--notional 774` | sizes agree within a percent |
| 47 | `--lots 1` BTC | 1,000,000,000 base units — **1 BTC, not 100,000** |
| 48 | **[FX]** `--lots 1` EUR/USD | 100,000,000,000,000 base units |
| 49 | **[FX]** `--lots 1` XAU/USD | 100,000,000,000 — 100 troy oz |
| 50 | `--notional 1000` twice, minutes apart | different base sizes as price moves |
| 51 | `--lots 0.07` | exact; a float would land a hair off |
| 52 | `--lots 0.0001` BTC | 100,000 base units, clears the dust floor |
| 53 | `--lots 0.000001` BTC | below the floor → rejected |
| 54 | Open `--notional 1000`, read `status` | notional ≈ $1,000 at entry |
| 55 | Compare `size_base × entry / 1e12` to notional | matches the fee/OI figures |

## 5. Closing (56–65)

| # | Command | Expect |
|---|---|---|
| 56 | Open one, `trade close --market BTC/USD` | closes without `--nonce` |
| 57 | Open two, `close` with no `--nonce` | refuses, names both nonces |
| 58 | `close --nonce 3` with only nonce 0 open | error naming what is open |
| 59 | `close` with nothing open | "no open position" |
| 60 | Close a long | slippage bound **below** oracle (selling) |
| 61 | Close a short | slippage bound **above** oracle (buying) |
| 62 | Close, then `status` | collateral returned minus fees, `open_positions` 0 |
| 63 | Open and close immediately | small loss — spread plus two fees |
| 64 | Close the same position twice | second fails, position closed |
| 65 | Close four positions in sequence | each returns its own collateral |

## 6. Oracle behaviour (66–75)

The gates that decide whether the protocol trades at all.

| # | Check | Expect |
|---|---|---|
| 66 | Stop the poster, wait 60s, open | `OracleStale` |
| 67 | Restart the poster, open | works again |
| 68 | Read a price account's byte 40 | `1` = `Full`; `0` = Partial and trades will fail |
| 69 | Confirm every posted feed is `Full` | all five |
| 70 | Compare market feed id to the price account's | identical, or `WrongOracleFeed` |
| 71 | Pass a *different* market's price account | `WrongOracleFeed` **[needs tooling]** |
| 72 | Compare validator clock to wall clock | drift; staleness is judged on chain time |
| 73 | Time a poster pass | ~16s for 5 markets; ~102s for 33 |
| 74 | `devnet-feed-probe --cluster devnet` | 8 of 33 usable — the sponsored-feed gap |
| 75 | `devnet-feed-probe --cluster mainnet` | far more; confirms it is a devnet gap |

## 7. Sessions and market status (76–83)

| # | Check | Expect |
|---|---|---|
| 76 | BTC/USD on a Saturday | trades — crypto is continuous |
| 77 | **[FX]** EUR/USD Saturday | refused |
| 78 | **[FX]** EUR/USD Monday 08:00 UTC | trades |
| 79 | **[FX]** Watch EUR/USD across 21:00 UTC Sunday | closed → open |
| 80 | XAU/USD on a weekend | refused; Pyth metals freeze Friday |
| 81 | Market in `Initialized` | `MarketClosedForOpens` |
| 82 | `set_market_status(ReduceOnly)` | closes allowed, opens refused **[needs tooling]** |
| 83 | `set_market_status(Halted)` | only liquidations **[needs tooling]** |

## 8. LP pool and accounting (84–88)

| # | Check | Expect |
|---|---|---|
| 84 | LP vault before and after a round trip | rises by the trader's loss |
| 85 | Trader loss vs LP gain | LP gain slightly less; the rest is fee splits |
| 86 | Open a position, watch OI | rises by notional; falls to zero on close |
| 87 | Long and short of equal size | OI on both sides, skew ≈ 0 |
| 88 | Exceed `max_oi_long` | rejected **[needs tooling: 1M cap]** |

## 9. Liquidation and keeper (89–94)

The hardest to drive by hand — liquidation needs the price to move against a thin position.

| # | Check | Expect |
|---|---|---|
| 89 | Open at maximum leverage | lands, health factor near the floor |
| 90 | Hold it while BTC moves ~1% adverse | approaches maintenance margin |
| 91 | Run `scripts/run-keeper.sh --dry-run` | logs candidates, sends nothing |
| 92 | Run the keeper for real on an underwater position | liquidates, pays the reward |
| 93 | Kill the keeper mid-run, restart | rebuilds from chain, no lost state |
| 94 | Two keepers at once | one wins; the loser logs a benign race |

## 10. Invariants under real conditions (95–100)

I1–I8 are asserted in the automated suite. These check they survive a live cluster.

| # | Check | Expect |
|---|---|---|
| 95 | Sum collateral vault vs sum of user collateral | vault ≥ deposits (I1) |
| 96 | LP vault balance vs `LpPool.aum` | equal (I2) |
| 97 | Fee vault after several trades | only ever rises (I3) |
| 98 | Every position has a live user account | no orphans (I4) |
| 99 | Every open position is within its leverage cap | (I5) |
| 100 | Open, close, and re-check 95–99 | all still hold |

---

## Quick regression set

Ten cases that would have caught every bug found so far:

```bash
trade setup --amount 100000                                              # 2
trade open  --market BTC/USD --direction long  --notional 1000 --collateral 200   # 11
trade status                                                             # 12
trade close --market BTC/USD                                             # 56
trade open  --market BTC/USD --direction long  --notional 500 --collateral 100 --nonce 0
trade open  --market BTC/USD --direction short --notional 500 --collateral 100 --nonce 1
trade close --market BTC/USD                                             # 57 — must refuse
trade close --market BTC/USD --nonce 0
trade close --market BTC/USD                                             # 56 — now unambiguous
trade open  --market BTC/USD --direction long --notional 100000 --collateral 100  # 26 — must reject
```

## Recording a failure

Note the case number, the full command, the **Anchor error name** (not just the hex), and
whether prices were fresh. The error name is the useful part: `SlippageExceeded` and
`PositionSizeOutOfBounds` look alike from the outside and have nothing in common.
