# 0 — Start to finish, both sides

Five actors. The **investor** and **trader** are wallets. **NOXFUNDS** is the rules program and
holds custody through PDAs. **SolFX** is the exchange it trades on, where the LP pool takes the
other side of every trade. The **keeper** is a bot anyone could run; every instruction it calls
is public.

```mermaid
sequenceDiagram
    autonumber
    actor T as Trader
    actor I as Investor
    participant N as NOXFUNDS program
    participant S as SolFX exchange + LP pool
    participant K as Keeper (anyone)

    rect rgba(120,120,255,0.08)
    Note over T,N: Part A - the trader earns a record (no investor money involved)
    T->>N: initialize_trader_profile (one per wallet, starts Bronze)
    T->>N: start_evaluation - 50 USDC stake into an escrow PDA
    loop Phase 1 (target 8 pct), then Phase 2 (target 5 pct)
        T->>N: eval_open_position / limit or stop entry (stop-loss mandatory)
        N->>S: reads the SolFX market and Pyth price, priced by SolFX's own code (nothing traded)
        K->>N: eval_trigger_stop / take-profit / fill entry / observe equity
        T->>N: eval_close_position
    end
    T->>N: claim_stage_pass (Phase 2)
    N-->>T: 50 USDC stake refunded, profile counts the pass
    Note over N: TraderVerified event - verified tick in the marketplace
    end

    rect rgba(120,255,120,0.08)
    Note over T,I: Part B - they find each other and agree
    T->>N: post_listing (what the trader is looking for)
    I->>N: post_offer - principal moves into offer escrow, rules fixed
    alt trader declines
        T->>N: decline_offer with a reason (no money moves)
        I->>N: revoke_offer - escrow returns to the investor
    else trader accepts
        T->>N: accept_offer - mandate created, escrow moves to mandate vault (one transaction)
    end
    end

    rect rgba(255,200,120,0.10)
    Note over T,S: Part C - the trader trades the investor's money
    T->>N: prepare - SOL for rent, create the SolFX account, move vault into SolFX collateral
    N->>S: create_solfx_account and deposit_collateral, signed by the mandate PDA
    T->>N: funded_open_position (rules checked first)
    N->>S: open_position + stop-loss, in the same transaction
    K->>N: observe_mandate_equity (breach check), reconcile_position (after a stop fires)
    T->>N: funded_close_position
    S-->>S: profit or loss settles against the LP pool, into the mandate's SolFX collateral
    end

    rect rgba(255,120,120,0.08)
    Note over I,S: Part D - it ends and everyone is paid
    I->>N: request_settlement (Active to WindingDown)
    K->>N: wind_down_position (anyone, if positions are still open)
    K->>N: claim_settlement (anyone)
    N->>S: withdraw all collateral back to the mandate vault
    N-->>I: principal + 30 pct of net profit (or everything left, on a loss)
    N-->>T: 70 pct of net profit (0 on a loss)
    N-->>N: 5 pct of gross profit to the treasury (0 on a loss)
    end
```

The same story in one line each:

| Step | Who signs | What moves |
|---|---|---|
| Evaluation | trader | 50 USDC stake into escrow, back on a pass, to the treasury on a fail |
| Offer | investor | the whole principal, investor's USDC account → offer escrow |
| Accept | trader | escrow → mandate vault. Trader pays the account rent |
| Prepare | anyone (the trader's page) | mandate vault → SolFX collateral |
| Trade | trader | collateral ↔ position margin. P&L comes from or goes to the SolFX LP pool |
| Settle | anyone | SolFX collateral → mandate vault → investor, trader, treasury |

The details of each part are in the numbered files that follow.
