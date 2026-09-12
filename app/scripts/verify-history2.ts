import { fetchHistory } from "../src/lib/history.js";
async function main() {
  const cfg = {
    hermesUrl: "https://pyth.dourolabs.app/hermes",
    token: process.env.PYTH_API_KEY,
  };
  const XAG =
    "f2fb02c32b055c805e7238d628e5e9dadef274376114eb1f012337cabe93871e";
  for (const [points, batch] of [
    [40, 5],
    [20, 5],
    [40, 2],
  ] as const) {
    const t0 = Date.now();
    const bars = await fetchHistory(cfg, XAG, points, 60, batch);
    console.log(
      `  points=${points} batch=${batch} -> ${bars.length} bars in ${Date.now() - t0}ms`
    );
  }
}
void main();
