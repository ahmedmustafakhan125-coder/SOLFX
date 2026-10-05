# 3 — The trader's evaluation: Phase 1, Phase 2, and the verified tick

An evaluation is how a trader builds a record **before** anyone risks money on them. The balance
is simulated; only the 50 USDC stake is real. Each simulated trade is priced by SolFX's own code
on the live Pyth price, so a simulated fill and a real one agree to the last unit.

## The whole journey

```mermaid
flowchart TB
    start(["Trader connects a wallet"]) --> prof["initialize_trader_profile<br/>one per wallet, starts at Bronze"]
    prof --> se["start_evaluation<br/>choose a simulated size, 10,000 to 200,000<br/>50 USDC stake moves into escrow"]
    se --> p1["Phase 1<br/>target +8 pct"]

    p1 --> trade1[["Trade (see the next diagram)"]]
    trade1 --> chk1{"Loss limit broken?<br/>more than 3 pct in a UTC day,<br/>or 6 pct below the peak"}
    chk1 -- yes --> fail["Failed"]
    chk1 -- no --> ready1{"claim_stage_pass allowed?<br/>target reached, 10+ trades over 5+ days,<br/>average hold 45+ min, no day above half the target,<br/>nothing open, no resting orders"}
    ready1 -- "not yet" --> trade1
    ready1 -- yes --> p2["Phase 2<br/>balance reset, target +5 pct"]

    p2 --> trade2[["Trade"]]
    trade2 --> chk2{"Loss limit broken?"}
    chk2 -- yes --> fail
    chk2 -- no --> ready2{"claim_stage_pass allowed?<br/>same conditions, 5 pct target"}
    ready2 -- "not yet" --> trade2
    ready2 -- yes --> pass["Passed<br/>stake refunded in full<br/>evaluations_passed + 1 on the profile<br/>TraderVerified event"]
    pass --> tick(["Verified tick next to the trader<br/>in the marketplace"])

    p1 -. "abandon_evaluation" .-> fail
    p2 -. "abandon_evaluation" .-> fail
    fail --> forfeit["forfeit_stake, anyone can call it<br/>50 USDC to the treasury"]
```

There is no time limit. The loss limits are judged whenever a close leaves the account flat, and
whenever anyone calls `eval_observe_equity`, which marks every open position to the live price
(the keeper does it every 5 minutes while positions are open). So refusing to close a losing
trade does not hide it.

## One simulated trade, and the orders a trader can use

```mermaid
flowchart TB
    subgraph IN["Getting in"]
        mk["Market order<br/>eval_open_position"]
        lim["Limit entry (new)<br/>eval_place_entry_order<br/>fills at the limit or better"]
        stp["Stop entry (new)<br/>fills once the price breaks through"]
    end

    rules{"Every open is checked:<br/>stop-loss present, on the right side,<br/>risk at the stop at most 1 pct of balance,<br/>at most 5 open, market open, prices fresh"}
    mk --> rules
    lim -- "keeper calls eval_fill_entry_order<br/>when the price reaches it" --> rules
    stp -- "same" --> rules
    rules -- fails --> refused["Refused - nothing opens.<br/>A resting order keeps waiting or expires"]
    rules -- passes --> pos["VirtualPosition<br/>entry, size, stop, optional take-profit"]

    subgraph MANAGE["While it is open"]
        tp["Set or move a take-profit (new)"]
        ms["Tighten the stop (new)<br/>only toward the price, only after 10 min"]
    end
    pos --- MANAGE

    subgraph OUT["Getting out"]
        cl["Close yourself<br/>allowed after 10 min"]
        sl["Stop-loss hit<br/>keeper: eval_trigger_stop<br/>exempt from the hold rules"]
        tpf["Take-profit hit (new)<br/>keeper: eval_trigger_take_profit<br/>after 10 min, counts as a normal close"]
    end
    pos --> cl
    pos --> sl
    pos --> tpf
    cl --> booked["P and L added to the simulated balance,<br/>trade counted on the record"]
    sl --> booked
    tpf --> booked
```

Why the stop can only be tightened, and only after 10 minutes: a stop-out is exempt from the
hold rules, because a stop is the discipline the rules ask for. If a trader could drag the stop
up to the price in minute one, that exemption would be a way to scalp. Same reason a take-profit
only fires after the 10-minute hold.

## What the verified tick means, and where it comes from

```mermaid
flowchart LR
    cs["claim_stage_pass, Phase 2"] --> ev["Evaluation state = Passed"]
    cs --> pr["TraderProfile.evaluations_passed + 1<br/>last_passed_at = now (new)"]
    cs --> e["TraderVerified event (new)"]
    ev --> mk["Marketplace shows the tick"]
    pr --> mk
    e --> ix[("Indexer")] --> mk
```

The tick is computed from the chain, not typed in by anyone: an evaluation account in state
`Passed`, or the counter on the trader's profile. The indexer re-derives the same thing from
events, and `/nox/verify` lets anyone check that the two agree.
