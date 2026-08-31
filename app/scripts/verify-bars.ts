import { fetchBars } from "../src/lib/history.js";
async function main() {
  const cfg = { hermesUrl: "https://pyth.dourolabs.app/hermes", token: process.env.PYTH_API_KEY };
  for (const [sym, id] of [
    ["BTC/USD", "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43"],
    ["XAG/USD", "f2fb02c32b055c805e7238d628e5e9dadef274376114eb1f012337cabe93871e"],
  ] as const) {
    const t0 = Date.now();
    const bars = await fetchBars(cfg, id, 12, 60, 3, 6);
    const up = bars.filter(b => b.close >= b.open).length;
    const withWick = bars.filter(b => b.high > b.low).length;
    console.log(`  ${sym.padEnd(8)} ${bars.length} bars in ${Date.now()-t0}ms  ${up} up / ${bars.length-up} down  ${withWick} with range`);
    const b = bars[bars.length-1];
    if (b) console.log(`           last O ${b.open.toFixed(4)}  H ${b.high.toFixed(4)}  L ${b.low.toFixed(4)}  C ${b.close.toFixed(4)}`);
  }
}
void main();
