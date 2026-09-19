#!/usr/bin/env python3
"""Refuse to commit or push a secret. This repository is public.

    python3 scripts/check-secrets.py --staged          # what is about to be committed
    python3 scripts/check-secrets.py --range A..B      # commits about to be pushed
    python3 scripts/check-secrets.py --all             # every commit, every branch

Wired in by `.githooks/pre-commit` and `.githooks/pre-push`. Enable once per clone:

    git config core.hooksPath .githooks

Two kinds of check, because they fail differently:

- **Paths.** A file that is a secret by *what it is* — `.env`, anything under `keys/`, a program
  keypair — is refused whatever it contains. `.gitignore` already excludes them; this catches
  the `git add -f` that overrides it.
- **Content.** Added lines are matched against the formats that have actually been at risk here:
  an RPC URL carrying its API key (the Helius key was pasted from a script's output once already),
  a Solana keypair as a 64-number array, PEM private keys, and the common token prefixes.

Matches are printed masked — the first six characters and a length — so the check that stops a
leak does not leak it into a terminal, a CI log or a screenshot.

It has a positive control: `--self-test` runs every pattern against a fabricated example of its
format and against the placeholders this repository legitimately uses, and fails if any real
format goes uncaught or any placeholder is flagged. A scanner that finds nothing is only
evidence of anything once it has been shown to find something.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys

PATTERNS: dict[str, re.Pattern[str]] = {
    # An API key as a query parameter, unless it is a visible placeholder or a shell variable.
    "rpc api key": re.compile(
        r"api[-_]?key=(?!<|\$|YOUR|your|redacted|\.\.\.|xxx|\{)[A-Za-z0-9_-]{8,}"
    ),
    "private key PEM": re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
    # `solana-keygen` writes a keypair as a JSON array of 64 byte values.
    "solana keypair": re.compile(r"\[\s*\d{1,3}\s*(?:,\s*\d{1,3}\s*){63}\]"),
    "anthropic key": re.compile(r"sk-ant-[A-Za-z0-9_-]{10,}"),
    "openai-style key": re.compile(r"\bsk-[A-Za-z0-9]{32,}"),
    "github token": re.compile(
        r"\b(?:ghp|gho|ghs|ghu)_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}"
    ),
    "aws key": re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    "slack token": re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{10,}"),
    "helius host+uuid": re.compile(
        r"helius[^\s\"']*[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
    ),
}

# Files that are secrets by what they are. `.env.example` is the template and is allowed.
FORBIDDEN_PATHS = re.compile(
    r"(^|/)\.env(\.(?!example$)[^/]*)?$"  # .env, .env.local — not .env.example
    r"|(^|/)keys/"  # program upgrade-authority keypairs
    r"|-keypair\.json$"  # target/deploy/<program>-keypair.json
    r"|(^|/)id\.json$"  # the Solana CLI's default wallet
    r"|\.(pem|key|p12|pfx)$"
)

# Fabricated examples of each format, for --self-test. Not real credentials.
#
# Every one is assembled from pieces at runtime, so no line of this file matches a pattern. That
# is not tidiness: this file passes through its own pre-commit hook, and a literal fake key here
# would be refused by it — and flagged by any scanner reading the public repository.
SAMPLES = {
    "rpc api key": "https://devnet.helius-rpc.com/?api-" + "key=1a2b3c4d-" + "FAKE-0000-0000-000000000000",
    "private key PEM": "-" * 5 + "BEGIN OPENSSH " + "PRIVATE KEY" + "-" * 5,
    "solana keypair": json.dumps(list(range(64))),
    "anthropic key": "sk-" + "ant-" + "api03-FAKEFAKEFAKEFAKE",
    "openai-style key": "sk-" + "a" * 40,
    "github token": "ghp_" + "A" * 36,
    "aws key": "AKIA" + "ABCDEFGHIJKLMNOP",
    "slack token": "xox" + "b-" + "1234567890-FAKE",
    "helius host+uuid": "helius-rpc.com/?k=" + "12345678-1234-" + "1234-1234-123456789abc",
}
PLACEHOLDERS = ["?api-key=<redacted>", '"$SOLFX_RPC_URL"', "api-key=YOUR_KEY_HERE"]


def mask(s: str) -> str:
    return f"{s[:6]}… ({len(s)} chars)"


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], capture_output=True, text=True, errors="ignore", check=True
    ).stdout


def scan_diff(diff: str) -> list[str]:
    """Findings in the added lines of a unified diff, plus any forbidden path it touches."""
    findings: list[str] = []
    commit, path = "", "?"
    for line in diff.split("\n"):
        if line.startswith("@@COMMIT "):
            commit = line.split()[1] + " "
            continue
        if line.startswith("+++ b/"):
            path = line[6:]
            if FORBIDDEN_PATHS.search(path):
                findings.append(f"{commit}{path}: a secret file by name")
            continue
        if not line.startswith("+") or line.startswith("+++"):
            continue
        for name, rx in PATTERNS.items():
            for m in rx.finditer(line):
                findings.append(f"{commit}{path}: {name} {mask(m.group(0))}")
    return findings


def self_test() -> int:
    bad = [n for n, s in SAMPLES.items() if not PATTERNS[n].search(s)]
    bad += [f"placeholder flagged: {p!r}" for p in PLACEHOLDERS if any(r.search(p) for r in PATTERNS.values())]
    bad += [f"path not refused: {p}" for p in [".env", "keys/a.json", "target/deploy/x-keypair.json", "id.json"] if not FORBIDDEN_PATHS.search(p)]
    bad += [f"path wrongly refused: {p}" for p in [".env.example", "clients/js/idl/noxfunds.json"] if FORBIDDEN_PATHS.search(p)]
    for b in bad:
        print(f"  self-test: {b}")
    print("  self-test: " + ("FAILED" if bad else f"ok — {len(SAMPLES)} formats caught, placeholders ignored"))
    return 1 if bad else 0


def main() -> int:
    mode = sys.argv[1] if len(sys.argv) > 1 else "--staged"
    fmt = "--format=@@COMMIT %h"
    if mode == "--self-test":
        return self_test()
    if mode == "--staged":
        diff = git("diff", "--cached", "--no-color", "--unified=0")
    elif mode == "--range":
        # Everything after the flag is a revision range, which may be several arguments —
        # the pre-push hook passes `<sha> --not --remotes` for a branch the remote has not seen.
        # Taking only the first would scan a different range and still report it clean.
        diff = git("log", "-p", "--no-color", fmt, *sys.argv[2:])
    elif mode == "--all":
        diff = git("log", "--all", "-p", "--no-color", fmt)
    else:
        print(__doc__)
        return 2

    findings = scan_diff(diff)
    if not findings:
        return 0
    print("\n  refused — this repository is public, and this looks like a secret:\n")
    for f in findings:
        print(f"    {f}")
    print(
        "\n  Remove it and put it in .env (ignored). If it is genuinely not a secret, say so in the"
        "\n  commit and bypass once with --no-verify — never as a habit.\n"
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
