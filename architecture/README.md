# Architecture diagrams

One flowchart per phase: what the phase added, which accounts it touches, and where the money
moves. Rendered PNGs for reading, Graphviz sources beside them so a change is reviewable as a
diff rather than as "494,388 bytes changed".

| Phase | Diagram | Source | Guide |
|---|---|---|---|
| 0 — Explainer and oracle feasibility | [phase-0.png](phase-0.png) | [phase-0.dot](phase-0.dot) | [guide](../docs/guides/phase-0-guide.md) |
| 1 — `solfx-math`, the financial engine | [phase-1.png](phase-1.png) | [phase-1.dot](phase-1.dot) | [guide](../docs/guides/phase-1-guide.md) |
| 2 — Vault, markets, Pyth pull oracle | [phase-2.png](phase-2.png) | [phase-2.dot](phase-2.dot) | [guide](../docs/guides/phase-2-guide.md) |
| 3 — Position engine | [phase-3.png](phase-3.png) | [phase-3.dot](phase-3.dot) | [guide](../docs/guides/phase-3-guide.md) |
| 4 — Risk engine and market regimes | [phase-4.png](phase-4.png) | [phase-4.dot](phase-4.dot) | [guide](../docs/guides/phase-4-guide.md) |
| 5 — LP vault | [phase-5.png](phase-5.png) | [phase-5.dot](phase-5.dot) | [guide](../docs/guides/phase-5-guide.md) |
| 6 — IB referral programme | [phase-6.png](phase-6.png) | [phase-6.dot](phase-6.dot) | [guide](../docs/guides/phase-6-guide.md) |
| 7 — Keepers | [phase-7.png](phase-7.png) | [phase-7.dot](phase-7.dot) | [guide](../docs/guides/phase-7-guide.md) |
| What comes next | [phase-next.png](phase-next.png) | [phase-next.dot](phase-next.dot) | [guide](../docs/guides/phase-next-guide.md) |

Re-render after editing a source:

```bash
dot -Tpng architecture/phase-3.dot -o architecture/phase-3.png
```
