# Keeping SolFX running

What has to be true for prices to reach the chain, what to do when it stops, and how to tell
which of the causes it is. Written after 23 September 2026, when prices sat 38 hours stale and
the health check reported "all healthy" 544 times.

## What has to be true

Three things, and a working API key is only one of them.

| | What | How to check |
|---|---|---|
| 1 | **A live Pyth API key** in `PYTH_API_KEY` in `.env` | `./scripts/set-pyth-key.sh --probe` |
| 2 | **SOL in the poster's fee payer**, `EyvqeDSh2Y4ZhobJY4bF8ueEZAjRx2V3r2GDf35ktPyo` | `solana balance EyvqeDSh2Y4ZhobJY4bF8ueEZAjRx2V3r2GDf35ktPyo --url devnet` |
| 3 | **The poster running** and completing passes that post something | `journalctl -u solfx-price-poster -n 20` |

The fee payer is shared with the keeper. It spends about **1.3 SOL a day** between them
(measured over a few minutes on 2026-09-23; re-measure over a day). **Keep at least 5 SOL in it
and top it up weekly.** The faucet refuses this server's IP, so fund it from your own wallet.

## Replacing the Pyth API key

Three services read the key, and each reads it **only when it starts**. Editing `.env` changes
nothing until all three restart:

| Service | What it uses the key for | If it misses the restart |
|---|---|---|
| `solfx-price-poster` | fetching every price it posts | prices go stale, markets halt |
| `solfx-keeper` | the price watchdog (compares the chain against Hermes) | the watchdog goes blind, quietly |
| `solfx-web` (container) | the browser's `/hermes` price proxy | the site's live prices stop when the old key dies |

**The one command:**

```bash
cd /root/SOLFX/SOLFX
./scripts/set-pyth-key.sh            # prompts for the key, hidden
```

In order, it: checks the new key against Hermes before touching anything, checks the fee payer
has SOL, backs up `.env`, writes the key, restarts all three, then waits for a pass that actually
posts. If none lands it says whether the fee payer is the reason.

**If you edited `.env` by hand instead**, restart all three yourself:

```bash
systemctl restart solfx-price-poster solfx-keeper
cd /docker/solfx && docker compose up -d      # the container re-reads env_file only on "up"
```

**Then check:**

```bash
./scripts/vps-health.sh
```

It says `running on an old Pyth key: <services>` if any of the three is still on the previous
key. That check exists because on 2026-09-23 the key was edited in and only the poster restarted:
the keeper and the site ran on the old key for a day while every other check said healthy. It
compares the keys inside the script and never prints them.

Markets halted by stale prices reopen by themselves: the keeper's session crank moves them
`Halted` → `GapWindow` → `Active`, about six minutes after prices resume.

## The NOXFUNDS crank

The keeper also runs NOXFUNDS' permissionless cranks (service `nox`, every 30 s; `--nox-secs` or
`SOLFX_NOX_SECS` to change it). In `journalctl -u solfx-keeper` it logs as `noxfunds` with an
`action`:

| action | when |
|---|---|
| `reconcile_position` | a funded position closed outside NOXFUNDS (its stop or take-profit fired, or it was liquidated) |
| `observe_mandate_equity` | an active mandate holding positions moved 25 bps of its peak since the last mark, or 5 minutes passed |
| `wind_down_position` | a breached or winding-down mandate still holds a position |
| `wind_down_cancel_stop` | a stopped mandate's stop outlived its position |
| `eval_trigger_stop` | an evaluation's simulated stop was reached |
| `eval_observe_equity` | an evaluation holds simulated positions and 5 minutes passed |
| `recompute_tier` | a trader's record earns a different tier from the one it shows |
| `eval_trigger_take_profit` † | a simulated target was reached and the position has been held 10 minutes |
| `eval_fill_entry_order` / `funded_fill_entry_order` † | a resting entry's trigger was reached on an open market; the program re-checks every rule |
| `eval_cancel_entry_order` / `funded_cancel_entry_order` † | an entry can no longer fill — expired, or its evaluation or mandate ended; the rent goes to the trader |
| `sweep_mandate_signer` † | a settled mandate's signer still holds SOL; it goes to the trader |

† From the 2026-10-03 upgrade; harmless before it (the keeper finds no such accounts). A fill the
chain refuses a moment later — the price moved back, or a limit's execution price is not there yet
— is logged at debug as "nothing to do, or lost a race", like a stop that stopped being met.

It does not settle mandates: `claim_settlement` is one click for anyone, and running it from here
would pay three parties at a moment none of them chose. It pays fees from the same wallet as the
poster.

## The NOXFUNDS indexer

`services/indexer/indexer.ts` keeps every NOXFUNDS transaction in SQLite and serves each trader's
record, evaluations, mandates and verified mark at `/api/nox/*`, through the site's own server
(`services/api/server.mjs`). It is **not installed**.

The site's server runs **inside the `solfx-web` container** (compose project `/docker/solfx`), not
as the `solfx-api` systemd unit, which has been disabled since 2026-09-07. Inside the container
`127.0.0.1` is the container itself, so the indexer listens on the `n8n_default` bridge gateway,
`172.18.0.1` (reachable from containers on that network and from this host, from nowhere else),
and the container is told where it is. To run it:

```bash
# 1. the indexer, on the host
cp services/indexer/solfx-nox-indexer.service /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now solfx-nox-indexer
curl -s 172.18.0.1:8788/api/nox/health          # lastSyncAt set once the first pass ends

# 2. the container: add under services.web.environment in /docker/solfx/docker-compose.yml
#      - NOX_INDEXER_URL=http://172.18.0.1:8788
cd /docker/solfx && docker compose up -d        # recreates solfx-web; a few seconds' gap
curl -s localhost:8787/api/nox/health           # the same answer, through the site
```

If the second `curl` says `the NOXFUNDS indexer is not running`, the container cannot see the
gateway address — check the network's gateway with
`docker network inspect n8n_default --format '{{range .IPAM.Config}}{{.Gateway}}{{end}}'`.

It reads the public devnet RPC at one call per 250 ms, deliberately not the keyed endpoint the
poster depends on. The first backfill took ~25 minutes on 2026-10-03 (515 transactions); it
caches what it fetches, so a restart resumes. `stored.truncated` counts transactions whose logs
were cut off — each trader's record says how many of its own it rests on.

To check it against the chain: `curl -s 'localhost:8788/api/nox/traders/<wallet>?verify=1'`
returns the record beside the profile account's own figures, field by field.

## Deploying the 2026-10-03 NOXFUNDS upgrade

From the WSL checkout (`/mnt/e/SOLFX`), after pulling:

1. **Precondition — no simulated position on devnet.** `VirtualPosition` grew; an existing one
   would no longer deserialise. Count them by the account's discriminator (`ZyLzBLz1sY1`); it must
   print `0` (it did on 2026-10-03). Close any open evaluation position first.

   ```bash
   curl -s https://api.devnet.solana.com -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,
     "method":"getProgramAccounts","params":["9B7qLbLk9PdRfiMEEK9Jzeen1nG8xzA7YvXsELS1DPUx",
     {"dataSlice":{"offset":0,"length":0},"filters":[{"memcmp":{"offset":0,"bytes":"ZyLzBLz1sY1"}}]}]}' \
     | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["result"]))'
   ```
2. `anchor build`, then `anchor upgrade` (or `./scripts/deploy-devnet.sh`) for `noxfunds` only.
   `solfx-core` and `solfx-referral` are unchanged.
3. Publish the IDL; `deploy-devnet.sh` compares it with `clients/js/idl/noxfunds.json`, which was
   built from the same source with Anchor's own `IdlBuilder`.
4. On the VPS, after the upgrade lands: rebuild and restart the keeper
   (`cargo build --release -p solfx-keeper && systemctl restart solfx-keeper`), then
   `npm run build` in `app/` — not before, or the site sends instruction layouts the live program
   does not know. The step-by-step version, with expected output, is
   [`NOXFUNDS-DEPLOY.md`](NOXFUNDS-DEPLOY.md).


`journalctl -u solfx-price-poster -n 40 -o cat`

| The log says | Cause | Fix |
|---|---|---|
| `no feed is readable with this API key` or an HTTP `401` / `403 Not entitled` | The key is missing or its grant ended. A dead key still authenticates: it returns **403**, not 401 | Replace the key (above) |
| `pass complete: 0 posted, 6 failed` with `init_encoded_vaa … was not confirmed in 45s` | **The fee payer is out of SOL.** The poster skips preflight, so an unpayable transaction is dropped without any error | Fund the fee payer; the poster recovers on its own within a pass or two |
| `429` | The RPC is rate-limiting | Lower `--max-rps` or use a paid RPC |
| No `pass complete` lines at all | The poster is not running | `systemctl status solfx-price-poster` |

A closed FX or metals market at the weekend posts successfully and is still stale — Pyth carries
Friday's last price forward. That is correct, not a fault; only BTC/USD trades at the weekend.

## Is it really fresh?

`publish_time` older than 60 seconds on a weekday means the program will refuse trades with
`OracleStale`. The health check (`./scripts/vps-health.sh`, every five minutes via
`solfx-health.timer`) now checks for a pass that *posted* and for the fee payer's balance. Before
2026-09-23 it counted `pass complete: 0 posted, 6 failed` as progress and skipped the balance
check entirely, which is how two days of stale prices went unreported.

## Getting told when it breaks

Alerts go to the systemd journal, which nobody reads. **Set `SOLFX_ALERT_WEBHOOK` in `.env`** to a
webhook URL (the n8n instance on this box, a Discord or Telegram webhook) and the health check will
POST every failure to it as JSON. Until it is set, the health check says so on every run.

## Things that are not the problem, however they look

- **Two posters look like one slow poster.** Check with `ps -eo pid,etime,cmd | grep '[d]ebug/price-poster\|[r]elease/price-poster'`.
- **`pgrep -f price-poster` matches its own shell.** Use the bracket form above.
- **The RPC gateway** (`solfx-rpc-gateway`, `127.0.0.1:8899`) reads `SOLFX_RPC_URL` only at start.
  If you change the RPC URL, restart it as well.
