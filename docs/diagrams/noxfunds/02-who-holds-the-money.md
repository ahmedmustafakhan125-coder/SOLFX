# 2 — Who holds the money, at every moment

The investor's USDC passes through four places and comes out in up to three. At no point is it
in an account that a person holds a key to. The vaults belong to PDAs, and PDAs have no key.

## The path of the investor's USDC

```mermaid
flowchart TB
    A["Investor's USDC account (ATA)<br/>owner: investor wallet"]
    B["Offer escrow<br/>PDA token account [offer_vault, offer]<br/>owner: the offer PDA"]
    C["Mandate vault<br/>PDA token account [vault, mandate]<br/>owner: the mandate signer PDA"]
    D["SolFX collateral vault (shared by all SolFX users)<br/>the mandate's share is free_collateral<br/>on SolFX UserAccount [user, mandate signer]"]
    E["Open position's margin<br/>still inside SolFX"]
    LP[("SolFX LP pool<br/>the counterparty")]
    F1["Investor's USDC account"]
    F2["Trader's USDC account"]
    F3["Treasury's USDC account"]

    A -- "1. post_offer<br/>investor signs" --> B
    B -- "1b. revoke_offer<br/>investor takes it back, any time before acceptance" --> A
    B -- "2. accept_offer<br/>trader signs, same transaction creates the mandate" --> C
    A -. "or fund_mandate<br/>investor and trader both sign" .-> C
    C -- "3. fund_solfx_collateral<br/>anyone, CPI deposit_collateral signed by the PDA" --> D
    D -- "4. funded_open_position" --> E
    E -- "5. close, stop, take-profit, wind-down" --> D
    E <-- "profit paid by the pool, loss paid to it" --> LP
    D -- "6. claim_settlement withdraws everything" --> C
    C -- "7a. principal + 30 pct of net profit" --> F1
    C -- "7b. 70 pct of net profit" --> F2
    C -- "7c. 5 pct of gross profit" --> F3
```

Steps 1 to 3 are the escrow. Steps 4 and 5 are trading. Steps 6 and 7 are settlement, in **one**
instruction. Nothing pays out until every position is closed.

## Who can move it out of each place

| Place | Address (seeds) | Owner | The only ways out |
|---|---|---|---|
| Investor's USDC account | the wallet's ATA | investor | the investor's own signature |
| Offer escrow | `["offer_vault", offer]` | offer PDA | `accept_offer` → mandate vault (trader signs) · `revoke_offer` → back to the investor (investor signs) |
| Mandate vault | `["vault", mandate]` | mandate signer PDA `["signer", mandate]` | `fund_solfx_collateral` → SolFX · `claim_settlement` → the three recorded payees |
| SolFX collateral | the mandate's SolFX account `["user", mandate signer]` | mandate signer PDA | trades (margin and P&L) · `claim_settlement` → the mandate vault. **There is no withdraw instruction for the trader.** |
| Payouts | USDC accounts whose owner is the investor, trader and treasury **recorded on the mandate** | them | theirs |

Two consequences worth reading twice:

- **Who gets paid is fixed when the mandate is created.** `claim_settlement` can be called by
  anyone, but it only accepts payee accounts owned by the investor and trader written on the
  mandate, and the configured treasury. A caller cannot send the money anywhere else.
- **Accept is atomic.** The mandate, its vault and the transfer are one transaction. There is no
  moment where terms are agreed and the money has not moved, or the money moved and there are
  no terms.

## Why a separate "mandate signer" PDA?

```mermaid
flowchart LR
    M["Mandate account<br/>holds the rules and state<br/>(has data)"]
    S["Mandate signer PDA [signer, mandate]<br/>holds no data, a little SOL"]
    V["Mandate vault<br/>USDC"]
    U["SolFX UserAccount<br/>and every SolFX position"]

    M -- "derives" --> S
    S -- "owns" --> V
    S -- "is the authority of" --> U
    S -- "pays rent for SolFX<br/>positions and stop orders" --> U
```

SolFX makes the account authority pay rent for new positions, through the System Program, and
the System Program refuses a payer that carries data. So the authority has to be a separate,
dataless PDA. That is why the trader's page has a **prepare** step that tops it up with a little
SOL. After settlement, `sweep_mandate_signer` **(new)** returns whatever SOL is left to the
trader.

## The trader's evaluation stake

```mermaid
flowchart LR
    T["Trader's USDC account"]
    EV["Stake escrow<br/>PDA token account [eval_vault, evaluation]<br/>owner: the evaluation PDA"]
    TR["Treasury's USDC account"]

    T -- "start_evaluation<br/>50 USDC" --> EV
    EV -- "claim_stage_pass, Phase 2<br/>refunded in full" --> T
    EV -- "forfeit_stake, anyone<br/>after a fail or abandon" --> TR
```

The simulated balance ($10,000 to $200,000) is a number on the `Evaluation` account. No real
money backs it and none moves when a simulated trade wins or loses. Only the stake is real.

## Who pays rent (SOL), and gets it back

| Account | Paid by | Returned |
|---|---|---|
| Offer and its escrow | investor | no, they stay as the public record of the negotiation |
| Mandate and its vault | trader on `accept_offer` (investor on `fund_mandate`) | no, it is the permanent record |
| SolFX positions | the mandate signer PDA (topped up by **prepare**) | to the signer when SolFX closes the position, swept to the trader after settlement |
| Evaluation, stake escrow | trader | no, it is the record |
| A simulated position | trader, or the keeper who fills an entry order | to the trader on close |
| A resting entry order **(new)** | whoever places it | to the keeper that fills it, or to the trader on cancel |
