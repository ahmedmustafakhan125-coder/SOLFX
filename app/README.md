# SolFX — Pro Terminal

React + Vite front end for `solfx-core`, built on `@solana/client`, `@solana/react-hooks`
and `@solana/kit`, consuming the SDK in [`clients/js`](../clients/js).

```bash
npm run dev      # http://localhost:5173
npm run build    # typecheck + bundle
```

Point it at a cluster with SolFX deployed:

```bash
echo 'VITE_SOLFX_RPC_URL=https://your-rpc' > .env.local
```

Devnet's public endpoint rate-limits hard enough to make the terminal look broken, so use a
provider endpoint.

## How it reads the venue

Nothing here restates the program's layout. Addresses come from the SDK's generated PDA
helpers and every field from a generated decoder, so a program change reaches the UI by
regenerating rather than by hand-editing.

`public/price-accounts.json` is the poster's own feed map, and it is load-bearing rather than
a convenience. Where nothing sponsors a Pyth feed, the price-update account is a keypair the
poster created, not the canonical `[shard, feed_id]` PDA — deriving the sponsored address
compiles, reads cleanly, and then talks to an account nobody writes to. Regenerate the file
whenever the poster does.

A raw Pyth mantissa is not a price either: feeds publish at different exponents (BTC/USD at
−8, EUR/USD at −5, XAU/USD at −3), and `lib/prices.ts` normalises each to the program's
`PRICE_PRECISION` of 1e9 before anything is displayed or sized against it.

## What is honest about it

The terminal shows the venue as it actually is, including when that is unflattering:

- markets that are **Halted** are listed under "Not trading" rather than hidden, because a
  client that silently drops an account cannot explain where a market went;
- a price older than the program's 60-second gate is badged **STALE**, and the order ticket
  refuses, because the program would reject that order too;
- leverage is capped at each market's own `max_leverage`, which differs per market — there is
  no protocol-wide figure;
- fees are shown in basis points from the market's own `open_fee_rate`, since the real
  figures round to nothing useful as percentages.

## Not wired yet

Order submission. Every figure in the ticket is computed with the program's own formulas from
live account data, but nothing is signed or sent.
