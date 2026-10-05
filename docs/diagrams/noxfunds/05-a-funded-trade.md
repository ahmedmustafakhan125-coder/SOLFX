# 5 — One trade on investor money

The trader never talks to the exchange. They sign a transaction **to NOXFUNDS**; NOXFUNDS checks
every rule and only then signs to SolFX, as the mandate's PDA. The trader's signature never
reaches SolFX, so there is no way round the checks.

## Opening: rules, position and stop-loss in one transaction

```mermaid
sequenceDiagram
    autonumber
    actor T as Trader
    participant N as NOXFUNDS
    participant S as SolFX
    participant P as Pyth price

    T->>N: funded_open_position(market, direction, size, margin, price limit, stop)
    Note over N: Checks, in order<br/>signer is this mandate's trader<br/>protocol not paused, mandate Active<br/>market in allowed_markets<br/>trade notional and total notional under the caps<br/>open positions under the limit<br/>stop present, right side, within max distance<br/>risk at the stop within max_risk_per_trade_bps
    alt any check fails
        N--xT: transaction reverts - nothing filled, nothing recorded, nothing lost
    else all pass
        N->>S: CPI open_position, signed by the mandate signer PDA
        S->>P: read the price (staleness and confidence gates)
        S-->>N: position opened, margin taken from the mandate's free collateral
        N->>S: CPI place_trigger_order - the stop-loss
        S-->>N: stop resting on SolFX
        N-->>T: done - position and stop exist together, or neither does
    end
```

Risk per trade is the rule a normal prop firm **cannot** enforce: it can only measure your risk
once you already hold the position. Here the stop is part of the same transaction, so the
program knows the worst case before anything fills.

## While it is open, and how it can end

```mermaid
flowchart TB
    pos(["Funded position open on SolFX"])

    subgraph TRADER["What the trader can do"]
        tp["Place a take-profit<br/>funded_place_take_profit, after the min hold"]
        mv["Move the stop (new)<br/>funded_move_stop - new stop placed, then the old one cancelled"]
        red["Close part of it (new)<br/>funded_reduce_position"]
        mar["Add or remove margin (new)<br/>funded_add_margin / funded_remove_margin"]
        cl["Close it<br/>funded_close_position, after the min hold"]
    end

    subgraph ANYONE["What happens without the trader"]
        sl["SolFX's keeper fires the stop or take-profit"]
        liq["SolFX liquidation, if margin runs out"]
        obs["observe_mandate_equity<br/>marks equity to the price, may Breach"]
        wd["wind_down_position<br/>only once Breached or WindingDown"]
    end

    pos --- TRADER
    pos --- ANYONE
    sl --> rec["reconcile_position<br/>the keeper records a close that<br/>happened outside NOXFUNDS"]
    liq --> rec
    cl --> book
    rec --> book
    wd --> book
    red -- "gross P and L only, the trade counts once when fully closed" --> book
    book["Recorded on the mandate and on the TraderProfile:<br/>realized P and L, win or loss, hold time"]
```

What the trader **cannot** do, because no instruction exists for it: withdraw, change a rule,
change who gets paid, or trade a market the mandate does not allow.

## Resting entry orders on a mandate (new)

```mermaid
sequenceDiagram
    actor T as Trader
    participant N as NOXFUNDS
    participant K as Keeper (anyone)
    participant S as SolFX

    T->>N: funded_place_entry_order (limit or stop entry, size, stop-loss, optional take-profit, expiry)
    Note over N: stored as MandateEntryOrder [morder, mandate, id]<br/>nothing traded yet
    loop every keeper pass
        K->>N: is the trigger reached? funded_fill_entry_order
        Note over N: every open rule checked again, at the fill price<br/>a limit never fills worse than its limit
        alt rules pass
            N->>S: open_position + stop-loss, signed by the PDA
        else rules fail or not reached
            N--xK: refused - the order keeps waiting
        end
    end
    T->>N: funded_cancel_entry_order (any time, or anyone once expired or the mandate ended)
```

An order is a request, not a reservation. If the mandate's equity, open positions or caps have
changed by the time the price arrives, the fill is refused for exactly the reasons a market order
would be.
