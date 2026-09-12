import { fetchHistory } from "../src/lib/history.js";
async function main() {
  const cfg = {
    hermesUrl: "https://pyth.dourolabs.app/hermes",
    token: process.env.PYTH_API_KEY,
  };
  for (const [sym, id] of [
    [
      "BTC/USD",
      "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43",
    ],
    [
      "EUR/USD",
      "a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b",
    ],
  ] as const) {
    const t0 = Date.now();
    const bars = await fetchHistory(cfg, id, 20, 60, 5);
    const ms = Date.now() - t0;
    if (bars.length === 0) {
      console.log(`  ${sym}  no bars`);
      continue;
    }
    const lo = Math.min(...bars.map((b) => b.value)),
      hi = Math.max(...bars.map((b) => b.value));
    console.log(
      `  ${sym.padEnd(8)} ${bars.length} bars in ${ms}ms  range ${lo.toFixed(5)} – ${hi.toFixed(5)}`
    );
  }
}
void main();
