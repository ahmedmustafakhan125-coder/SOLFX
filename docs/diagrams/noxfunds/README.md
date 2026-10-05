# NOXFUNDS, in diagrams

NOXFUNDS is a prop firm with no firm. A trader proves themselves in a simulated evaluation; an
investor funds a trader they choose; the trader trades the investor's USDC on SolFX, under rules
the program checks **before** each trade fills. At the end the program divides the result
between them. Nobody holds a key to the money while that happens. Not the trader, not the
investor, not the admin.

Read in this order. Each file is one question.

| | File | The question it answers |
|---|---|---|
| 0 | [00-end-to-end.md](00-end-to-end.md) | What happens, start to finish, for both people? |
| 1 | [01-what-is-different.md](01-what-is-different.md) | Why is this not just a prop firm on a blockchain? |
| 2 | [02-who-holds-the-money.md](02-who-holds-the-money.md) | Where is the USDC at each moment, and who can move it? (escrow, vaults, accounts) |
| 3 | [03-trader-evaluation.md](03-trader-evaluation.md) | How does a trader pass Phase 1 and 2 and get the verified tick? |
| 4 | [04-investor-journey.md](04-investor-journey.md) | How does an investor find a trader, agree terms and fund them? |
| 5 | [05-a-funded-trade.md](05-a-funded-trade.md) | What happens inside one trade on investor money? |
| 6 | [06-settlement.md](06-settlement.md) | How does it end, and who gets paid what? |

Every name in a diagram (`accept_offer`, `["vault", mandate]`, `Breached`) is the real
instruction, account seed or state in [`programs/noxfunds/`](../../../programs/noxfunds/src/).
Marked **(new)**: built and tested, part of the upgrade in
[NOXFUNDS-DEPLOY.md](../../NOXFUNDS-DEPLOY.md), **not yet on devnet**.

## How to view them

- **GitHub** draws them: open any file on github.com.
- **VS Code**: install the *Markdown Preview Mermaid Support* extension
  (`bierner.markdown-mermaid`), then open a file and press `Ctrl+Shift+V`.
- **Any browser, all at once**: open [flowcharts.html](flowcharts.html). It needs an internet
  connection the first time, for the Mermaid library.

## Six words used everywhere

| Word | Meaning here |
|---|---|
| **PDA** | An address the program derives from fixed words ("seeds"). It has **no private key**: only the program it belongs to can sign for it, and only by running its own code. |
| **Vault** | A USDC token account whose owner is a PDA. NOXFUNDS' vaults are PDAs at fixed seeds, **not** ATAs, so nobody can substitute another account. |
| **ATA** | A wallet's ordinary USDC account (Associated Token Account). The money starts in the investor's and ends in the investor's, trader's and treasury's. |
| **CPI** | One program calling another inside the same transaction. NOXFUNDS calls SolFX this way, signing as a PDA. |
| **Crank / keeper** | A public instruction anyone may call (and the bot that does), to mark prices, fire stops, fill orders or wind down. None of them can move money anywhere it was not already going. |
| **Atomic** | A Solana transaction succeeds completely or not at all. A trade that breaks a rule is not punished afterwards: it never happens. |
