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

```bash
cd /root/SOLFX/SOLFX
./scripts/set-pyth-key.sh            # prompts for the key, hidden
```

In order, it: checks the new key against Hermes before touching anything, checks the fee payer
has SOL, backs up `.env`, writes the key, restarts the poster, the keeper and the web tier (each
reads `.env` only when it starts — editing the file alone changes nothing), then waits for a pass
that actually posts. If none lands it says whether the fee payer is the reason.

Markets halted by the stale prices reopen by themselves: the keeper's session crank moves them
`Halted` → `GapWindow` → `Active`, about six minutes after prices resume.

## When prices go stale — reading the poster's log

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
