# Task 4 — finish `anchor coverage` on the local machine

Everything below the line is the work to do on the WSL box. It is written to be
self-contained: it states the constraints, the known failure, the diagnosis, and what
"done" means, so none of it has to be rediscovered.

---

## The job

Produce SBF source coverage for `programs/solfx-core/src/instructions/` and record it.
This is the last open half of Phase 9 Task 4.

Working directory is `.`. Read `docs/CONTEXT.md` and the project notes first; they
outrank anything here.

## Where things stand

- `cargo test --workspace` is green: **555 passing**, of which `solfx-core` is **240**.
- `cargo llvm-cov` has already been run on the VPS for the two crates it can measure, and
  the output is committed at `docs/coverage/`:
  - `solfx-math` — 95.67% regions / 98.67% lines / 99.61% functions
  - `solfx-keeper` — 18.60% regions / 16.51% lines / 23.15% functions
- **`programs/solfx-core/src/instructions/` is measured at nothing, and `cargo llvm-cov`
  cannot measure it.** llvm-cov instruments a *host* build; the tests execute the compiled
  `.so` as SBF bytecode inside LiteSVM, so the instrumented copy is never run.
  `anchor coverage` is the tool that can: it builds with DWARF, runs the LiteSVM tests with
  SBF register tracing, maps executed program counters back to source lines, and emits LCOV.
- `programs/solfx-core/Cargo.toml` has already been edited by hand to
  `litesvm = { version = "0.15.2", features = ["register-tracing"] }`. Without that feature
  `SBF_TRACE_DIR` is silently ignored and coverage reports 0% rather than erroring.
  **This edit is not yet committed — commit it.**

## The failure to fix

Run from `./programs/solfx-core`, `anchor coverage` built and ran all 240 tests
successfully, then died immediately after the doc-test line with:

```
Error: Permission denied (os error 13)
```

No path, no backtrace. It fails at trace collection / LCOV writing, not at build or test.

### The two facts that most likely explain it

Both are already documented in the project rules:

1. **`./target` is a symlink** to `~/.cargo-target/solfx`, because `/mnt/e` is a
   9p mount and small-file I/O across it is ~200x slower. But
   `programs/solfx-core/target/` is *not* that symlink — anything Anchor writes relative to
   the crate directory lands on the slow Windows mount instead.
2. **`chmod` does not stick on `/mnt/e`.** drvfs approximates permission semantics, and
   creating directories with a mode it cannot represent is exactly where `EACCES` appears.

Coverage writes many small trace files, so it is the worst possible workload for that mount.

## Do this

**1. Diagnose before changing anything.** Paste the output into the transcript:

```bash
cd .
ls -ld target target/coverage target/coverage/traces 2>&1
ls -ld programs/solfx-core/target 2>&1
find target/coverage -maxdepth 2 ! -user "$(whoami)" -printf '%u %p\n' 2>/dev/null | head
mount | grep -E ' /mnt/e '
```

If that `find` reports root-owned files, an earlier `sudo` run left them and they must go:

```bash
sudo rm -rf ~/.cargo-target/solfx/coverage ./programs/solfx-core/target
```

Remove **only** the coverage output. Do not delete `target/deploy` — that holds the
program keypair, which is the program's permanent on-chain identity.

**2. Re-run with both paths on the Linux filesystem**, so neither the crate-relative
`target/` nor the 9p mount is involved:

```bash
cd .
ps aux | grep -q "[s]olana-test-validator" && echo "KILL THE VALIDATOR FIRST" || echo ok
mkdir -p ~/solfx-cov/traces

RUST_BACKTRACE=1 anchor coverage \
  --trace-dir "$HOME/solfx-cov/traces" \
  --output    "$HOME/solfx-cov/sbf.lcov"
```

If it still fails, capture the full backtrace and work the actual path out of it rather
than guessing again. Useful flags while iterating: `--skip-build` when the artifacts are
fresh, `--skip-run` to regenerate LCOV from traces already collected.

**3. Read the result.**

```bash
lcov --summary ~/solfx-cov/sbf.lcov        # apt install lcov if missing
genhtml ~/solfx-cov/sbf.lcov -o ~/solfx-cov/html   # optional, browsable
```

**4. Record it.** Copy the LCOV and a plain-text summary into `docs/coverage/`, matching
the two files already there. Update the Task 4 row in `docs/phase-9-report.md` and the
test/lint baseline table in `docs/CONTEXT.md` with the real percentage.

**5. The part that is actually worth something.** A percentage on its own is a number.
What Task 4 is for is finding which branches the 240 tests never enter. Go through the
uncovered lines in `programs/solfx-core/src/instructions/` and write up, in the report:

- which `require!` refusal arms are never exercised — these matter most, because a venue
  that accepts everything is not a venue, and an unexercised refusal is an untested one;
- whether all four `TriggerKind::is_met` direction/kind combinations are hit;
- the liquidation boundary, where equity exactly equals maintenance margin (the comparison
  is a strict `<`, so equality is deliberately *not* liquidatable);
- any instruction of the 38 with no coverage at all.

List them as findings. Do not write new tests in this session unless asked — the
deliverable is the measurement and the gap analysis.

## Constraints — these are not negotiable

- **Never run `cargo` while a validator is up.** Check with
  `ps aux | grep -q "[s]olana-test-validator"`, not `pgrep` (which gets this wrong in both
  directions: the process name exceeds pgrep's 15-character limit, and `pgrep -f` matches
  its own shell).
- **Do not modify anything under `programs/`** beyond the `litesvm` dev-dependency line
  that is already there. The local settings deny it mechanically. If coverage
  genuinely requires a program change, say so and stop.
- **Never run `anchor deploy`, `anchor upgrade` or `solana program deploy`.**
- Consult the `solana-mcp` before answering anything version-specific about Anchor,
  LiteSVM or the coverage tooling. Say so plainly if it has nothing.
- Verify, don't assert. Report the measurement, not the expectation.

## Definition of done

1. `~/solfx-cov/sbf.lcov` exists and `lcov --summary` reports a non-zero figure for
   `programs/solfx-core/src/instructions/`. A 0% result means the traces were not written —
   that is a failure to fix, not a result to record.
2. LCOV and summary committed under `docs/coverage/`.
3. `programs/solfx-core/Cargo.toml`'s `register-tracing` change committed.
4. `docs/phase-9-report.md` and `docs/CONTEXT.md` updated with the real number.
5. The gap analysis from step 5 written into the report.

## Branch

The VPS session has pushed `fix/ticket-sizing-and-read-commitment` (7 commits: order-ticket
sizing, read commitment, the positions-table header, the floating activity panel, load
performance, and doc corrections). `git pull` and commit onto that branch so the two
machines do not diverge.
