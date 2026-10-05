# 4 — The investor's journey: finding a trader, agreeing, funding

There is no chat. Every "message" is a typed on-chain object tied to a real step, carrying a short
note (180 bytes at most). The investor's offer **is** the contract: the money is already in
escrow and the rules are already fixed in it. The trader's acceptance is the signature.

## Finding each other

```mermaid
flowchart TB
    subgraph DISC["Discovery - /nox/market"]
        tl["Trader listing<br/>post_listing - what they are looking for"]
        il["Investor listing<br/>post_investor_listing - what they offer"]
        rec["Each trader's record:<br/>tier, profit factor, drawdown, trades,<br/>verified tick, all from the chain"]
    end

    inv(["Investor"]) --> rec
    rec --> choose{"Who starts?"}
    choose -- "investor" --> off["post_offer<br/>principal into escrow + the full rule set + expiry + note"]
    choose -- "trader, only to an investor<br/>with an open listing" --> req["post_request<br/>a note, one open request per pair"]
    req --> ans{"Investor answers"}
    ans -- "with an offer" --> off
    ans -- "dismisses" --> rclosed["close_request<br/>trader's rent returned"]

    off --> tr{"Trader decides"}
    tr -- "accept_offer" --> mandate["Mandate created, Active<br/>escrow moved into the mandate vault<br/>rules copied byte for byte from the offer"]
    tr -- "decline_offer, with a reason" --> dec["Declined - money still in escrow"]
    dec --> rev["revoke_offer<br/>escrow back to the investor"]
    dec -. "investor revises" .-> off
    off -- "investor changes their mind,<br/>or it expires" --> rev
```

What this guarantees:

- **The investor can always take the money back until a trader signs.** Declining never moves
  money, and an expired offer cannot be accepted.
- **A request can only reach an investor who has listed**, and only one per trader–investor
  pair. No wallet can be spammed for merely existing.
- **The rules the trader accepts are exactly the rules the investor published.** The program
  copies them from the offer; nobody retypes them.
- A trader can only accept up to their tier's limits: Bronze $10,000 and one mandate at a
  time, up to Platinum $200,000 and five.

## What the investor sets, and can never change afterwards

| Rule on the mandate | What it stops |
|---|---|
| `max_trade_notional` | one oversized bet |
| `max_total_notional` | many bets adding up to one |
| `max_concurrent_positions` | too many open at once |
| `max_risk_per_trade_bps` | a stop so far away it is not a stop (risk measured at the stop, before the fill) |
| `max_stop_distance_bps` | the same, measured in price |
| `max_drawdown_bps`, `max_daily_loss_bps` | trading on after the capital is impaired |
| `allowed_markets` | instruments outside the agreement |
| `min_hold_slots` | scalping (stop-outs exempt) |
| `trader_split_bps` | the trader's share of net profit (70 pct by default) |

There is no instruction that edits any of these. Different rules mean a different mandate.

## After acceptance: prepare, trade, watch, end

```mermaid
flowchart TB
    acc["Mandate Active<br/>USDC in the mandate vault"] --> prep["Prepare - one click on the trader's page, anyone may send it<br/>1. a little SOL to the mandate signer, for SolFX rent<br/>2. create_solfx_account<br/>3. fund_solfx_collateral - vault into SolFX"]
    prep --> trading["Trader trades - see 05-a-funded-trade.md"]
    trading --> watch{"Keeper or anyone:<br/>observe_mandate_equity"}
    watch -- "drawdown or daily-loss limit crossed" --> br["Breached<br/>no new trades"]
    watch -- "fine" --> trading
    trading --> rs["Investor: request_settlement<br/>Active to WindingDown, no new trades"]
    br --> wd["Positions still open?<br/>wind_down_position - anyone may close them"]
    rs --> wd
    wd --> cs["claim_settlement - anyone<br/>see 06-settlement.md"]
    cs --> done(["Settled"])
```

The investor never depends on the trader to get the money back. A breached or winding-down
mandate can be closed out and settled by anyone, with neither the trader nor any operator
cooperating.

## The two state machines

```mermaid
stateDiagram-v2
    direction LR
    state "Offer" as O {
        [*] --> Open: post_offer
        Open --> Accepted: accept_offer
        Open --> Declined: decline_offer
        Open --> Revoked: revoke_offer
        Declined --> Revoked: revoke_offer
        Accepted --> [*]
        Revoked --> [*]
    }
```

```mermaid
stateDiagram-v2
    direction LR
    [*] --> Active: accept_offer or fund_mandate
    Active --> Breached: observe_mandate_equity finds a limit crossed
    Active --> WindingDown: request_settlement (investor)
    Breached --> Settled: claim_settlement, once flat
    WindingDown --> Settled: claim_settlement, once flat
    Settled --> [*]
```

`Breached` does not pass through `WindingDown`. That keeps the record of **why** the mandate
ended, which is what the next investor reading this trader's history needs to see.
