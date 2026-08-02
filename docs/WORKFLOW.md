# Working on SolFX — Where Things Are and How to Run Them

**Your project lives here:**

```
.
```

That is your **E: drive**, reached from inside WSL. It was moved off the C: drive because C: was at
84% capacity and this project will grow large — E: has ~596 GB free.

> ### ⚠️ One trade-off you are accepting by living on E:
>
> `/mnt/e` is a Windows drive mounted into Linux through the 9p filesystem, and small-file I/O
> across that boundary is **roughly 200x slower** than the native Linux filesystem. `cargo` and
> `anchor` do enormous amounts of small-file I/O.
>
> **Mitigation:** the build *output* directory is redirected to native Linux disk via
> `CARGO_TARGET_DIR`. Source lives on E: (safe, roomy, visible from Windows); build artefacts —
> which are the bulk of both the size and the I/O — live on the fast Linux side and are disposable.
> See "Build speed" below.

There are three ways to reach the project. Use whichever suits the moment.

---

## 1. VS Code — your main way in

This is how you should do all editing.

**Open the project:**

1. Open VS Code
2. Press `Ctrl + Shift + P`
3. Type `WSL: Open Folder in WSL` and press Enter
4. Choose `Ubuntu` if asked
5. Navigate to `.` and click OK

The bottom-left corner of VS Code will show a green badge reading **`WSL: Ubuntu`**. That badge
means you're connected properly. Everything then behaves exactly as normal — the file tree, search,
git panel, extensions. The only difference is that the built-in terminal (`` Ctrl + ` ``) is now a
Linux shell, which is what you want.

> **First time only:** if `WSL: Open Folder in WSL` doesn't appear, install the **WSL** extension by
> Microsoft from the Extensions panel (`Ctrl + Shift + X`, search "WSL"), then retry.

**Shortcut — open it straight from a WSL terminal:**

```bash
cd .
code .
```

---

## 2. Windows Explorer — for browsing and drag-and-drop

Paste this into the Explorer address bar:

```
E:\SOLFX
```

That's it — no `\\wsl$\` path needed any more, because the project is on a real Windows drive.
Files open normally. Handy for grabbing a PDF or looking at a CSV in Excel.

**Pin it:** right-click the folder → *Pin to Quick access*.

---

## 3. Terminal — for running things

Open a WSL shell any of these ways:

- In VS Code: `` Ctrl + ` `` (once connected via WSL, as above) — **easiest**
- Windows Terminal → dropdown arrow → **Ubuntu**
- Start menu → **Ubuntu**
- Any Windows terminal → type `wsl`

Then:

```bash
cd .
```

---

## Build speed — read this once

Rust builds generate tens of thousands of small files. On `/mnt/e` that is painfully slow. The fix
is already configured in `.cargo/config.toml`:

```toml
[build]
target-dir = "$HOME/.cargo-target/solfx"
```

Source code stays on E:. Build output goes to native Linux disk. You get the roominess of E: and
the speed of the Linux filesystem at the same time.

**If a build ever feels wrong**, confirm the redirect is active:

```bash
cargo metadata --format-version 1 --no-deps | grep -o '"target_directory":"[^"]*"'
# should print $HOME/.cargo-target/solfx
```

**To reclaim disk:** `rm -rf $HOME/.cargo-target/solfx` — it is pure build output and is
regenerated on the next `cargo build`. Nothing of yours is in there.

---

## Everyday commands

All of these assume you are in `.`.

### Looking around

```bash
cd .          # go to the project
ls -la                   # list everything, with detail
tree -L 2                # folder tree (install once: sudo apt install tree)
pwd                      # print where you currently are
```

### Reading the docs

```bash
code docs/FOREX-EXPLAINED.md      # open in VS Code  <- best
less docs/ARCHITECTURE.md         # read in terminal (q to quit, / to search)
wc -l docs/*.md                   # how long each doc is
```

### The oracle probe

```bash
cd ./scratch/feed-probe

pgrep -f '[p]robe\.py'     # is it alive? prints a number = yes, nothing = no
bash run-probe.sh          # start it (safe to re-run, won't double-start)
pkill -f '[p]robe\.py'     # stop it

python3 analyse.py         # the report — run this any time
python3 analyse.py --markdown   # same, as a markdown table
python3 check-silent.py    # check feeds that aren't reporting

tail -f probe.log          # watch it live (Ctrl+C to stop watching)
wc -l data/observations-*.csv   # how much data collected so far
```

> **Note the square brackets.** `pkill -f probe.py` (without them) also matches the shell you typed
> it into, so it kills your own terminal. `'[p]robe\.py'` matches the probe and nothing else.

> **Important:** the probe dies if you shut down Windows or run `wsl --shutdown`. That's fine —
> just run `bash run-probe.sh` again and it picks up where it left off.
>
> It died this way on **29–31 July**, leaving a 41.4-hour hole in the data. `analyse.py` now
> collects BTC/USD and SOL/USD as liveness controls: crypto never closes, so if the controls have a
> gap the probe was down, and if only the FX feeds gap the market was closed. Before that, the two
> were indistinguishable — which is what produced the incorrect "LATAM feeds are hourly" finding.

### Git

```bash
git status               # what's changed
git log --oneline        # history
git add -A               # stage everything
git commit -m "message"  # commit
git push                 # send to GitHub
git diff                 # see your changes
```

### Building and testing (Phase 1 onward)

```bash
cargo build              # compile Rust
cargo test               # run tests
cargo test -p solfx-core # test just the core program
cargo fmt                # auto-format code
cargo clippy -- -D warnings   # lint (catches bugs); CI fails on any warning

anchor build             # compile the Solana program
anchor test              # run the full test suite
solana-test-validator    # start a local blockchain
```

---

## Current layout

```
./
├── README.md                        project overview
├── Cargo.toml                       Rust workspace root
├── rust-toolchain.toml              pins the exact compiler version
├── .cargo/config.toml               redirects build output to fast disk
├── docs/
│   ├── FOREX-EXPLAINED.md           plain English — START HERE
│   ├── ARCHITECTURE.md              the full technical spec
│   ├── oracle-feasibility.md        Phase 0b findings
│   └── WORKFLOW.md                  this file
├── programs/
│   ├── solfx-core/                  the financial engine
│   └── solfx-referral/              the IB rebate program
├── tests/                           integration tests
└── scratch/
    └── feed-probe/                  the oracle measurement probe
```

---

## Two things that will save you confusion

**1. Two filesystems, one machine.** Windows drives (`C:`, `E:`) appear inside WSL under `/mnt/c`,
`/mnt/e`. WSL's own Linux disk is `$HOME`. Your *source* is on E: (roomy, backed up by being
on a real drive, visible from Windows). Your *build output* is on the Linux disk (fast, disposable).

**2. `~` means your Linux home folder** — `$HOME`. That is **not** where the project is any
more. The project is at `.`. `cd ~` takes you home; `cd .` takes you to work.

---

## If something looks wrong

```bash
# "I can't find my files"
ls .                   # they're here

# "Did I lose my work?"
cd . && git status     # git tracks everything

# "Is the probe still collecting?"
pgrep -f '[p]robe\.py'            # number = yes, nothing = restart it

# "Which folder am I in?"
pwd

# "Is my toolchain OK?"
rustc --version && solana --version && anchor --version && node --version

# "Builds are crawling"
cargo metadata --format-version 1 --no-deps | grep -o '"target_directory":"[^"]*"'
# must NOT be under /mnt/e — see "Build speed" above
```
