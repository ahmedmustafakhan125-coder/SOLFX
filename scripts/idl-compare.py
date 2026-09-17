#!/usr/bin/env python3
"""Compare a published IDL with a local build by the interface a client depends on.

    scripts/idl-compare.py ON_CHAIN.json LOCAL.json

Exits 0 when they describe the same interface and 1 when they do not, printing every
difference it finds.

# Why names are not enough

The first version of this check compared the *set of instruction names*. Adding the
initialisation guard to NOXFUNDS's `initialize_config` changed it from three accounts to five
and renamed nothing, so that check would have reported the stale on-chain IDL as current and
never republished it. Measured on devnet on 2026-09-17: the published IDL listed
`[admin, config, system_program]`; the build had five. Any client building from the on-chain
IDL would have sent a transaction the program rejects.

So this compares what a client actually encodes: each instruction's discriminator, accounts
(including signer and writable flags) and arguments; account and event discriminators; error
codes; the type definitions; and the address. `docs` is ignored because `publish-idl.sh`
strips it before writing. `metadata` is ignored because it names the build, not the interface.
"""

import json
import sys


def strip_docs(node):
    if isinstance(node, dict):
        return {k: strip_docs(v) for k, v in node.items() if k != "docs"}
    if isinstance(node, list):
        return [strip_docs(v) for v in node]
    return node


def by(items, key):
    return {item[key]: item for item in items or []}


def differences(on_chain, local):
    a, b = strip_docs(on_chain), strip_docs(local)
    out = []

    if a.get("address") != b.get("address"):
        out.append(f"address: on chain {a.get('address')}, local {b.get('address')}")

    for section, key in (
        ("instructions", "name"),
        ("accounts", "name"),
        ("events", "name"),
        ("errors", "code"),
        ("types", "name"),
    ):
        have, want = by(a.get(section), key), by(b.get(section), key)
        for name in sorted(set(want) - set(have), key=str):
            out.append(f"{section}: {name} is missing on chain")
        for name in sorted(set(have) - set(want), key=str):
            out.append(f"{section}: {name} is on chain but not in the build")
        for name in sorted(set(have) & set(want), key=str):
            if have[name] == want[name]:
                continue
            fields = sorted(
                f
                for f in set(have[name]) | set(want[name])
                if have[name].get(f) != want[name].get(f)
            )
            detail = ""
            if section == "instructions" and "accounts" in fields:
                detail = (
                    f" (on chain {[x.get('name') for x in have[name].get('accounts', [])]},"
                    f" build {[x.get('name') for x in want[name].get('accounts', [])]})"
                )
            out.append(f"{section}: {name} differs in {', '.join(fields)}{detail}")
    return out


def main():
    if len(sys.argv) != 3:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        sys.exit(2)
    with open(sys.argv[1]) as f:
        on_chain = json.load(f)
    with open(sys.argv[2]) as f:
        local = json.load(f)

    diffs = differences(on_chain, local)
    if diffs:
        print(f"  stale: {len(diffs)} difference(s)")
        for d in diffs:
            print(f"    {d}")
        sys.exit(1)
    print(f"  on-chain IDL matches the build: {len(local.get('instructions', []))} instructions")


if __name__ == "__main__":
    main()
