import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

const sdk = fileURLToPath(new URL("../clients/js/src", import.meta.url));

/**
 * Hermes' bearer token, read from the repo root .env at config time.
 *
 * It is used by the proxy below and never reaches the browser bundle, which is the point:
 * a key in client JavaScript is a public key.
 */
function hermesToken(): string {
  try {
    const env = readFileSync(fileURLToPath(new URL("../.env", import.meta.url)), "utf8");
    return /^PYTH_API_KEY=(.*)$/m.exec(env)?.[1]?.trim() ?? "";
  } catch {
    return "";
  }
}

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      // The SDK is consumed as TypeScript source rather than a build artefact. There is no
      // compile step to keep in sync, and the generated client stays a single source of
      // truth for both this app and the Node scripts.
      { find: /^@solfx\/client$/, replacement: `${sdk}/index.ts` },
      { find: "@", replacement: fileURLToPath(new URL("./src", import.meta.url)) },
    ],
  },
  optimizeDeps: {
    // The SDK is consumed as source, so Vite watches it rather than pre-bundling it. But a
    // package left out of pre-bundling resolves its own copy of anything it imports, and two
    // copies of @solana/kit means two module registries and mismatched instanceof checks —
    // the hazard Kit's own example config documents. Forcing kit to be pre-bundled makes it
    // one shared copy that both the app and the SDK reference.
    exclude: ["@solfx/client"],
    include: ["@solana/kit", "@solana/client", "@solana/react-hooks"],
  },
  server: {
    // The repo lives on /mnt/e, a 9p mount, and inotify events do not cross it. Without
    // polling, Vite's watcher never fires: the dev server keeps serving the transform it
    // built at startup, so edits appear to do nothing and a module deleted since startup
    // still answers 200. A half-stale module graph is also the most likely explanation for
    // the blank page — an import that no longer matches what the rest of the graph expects
    // throws before anything renders. Polling costs a little CPU and buys a working reload.
    watch: { usePolling: true, interval: 400 },
    fs: { allow: [fileURLToPath(new URL("..", import.meta.url))] },
    proxy: {
      // Hermes answers the CORS preflight but omits `access-control-allow-origin` on the
      // actual response, so a browser fetch is blocked even though curl and Node succeed.
      // Proxying through the dev server sidesteps that and keeps the API key server-side.
      "/hermes": {
        target: "https://pyth.dourolabs.app",
        changeOrigin: true,
        headers: { Authorization: `Bearer ${hermesToken()}` },
      },
      // Pyth Pro's History API — real OHLC at every resolution TradingView understands, on
      // the same host and the same key. Proxied for the same two reasons as Hermes: the
      // browser cannot read a cross-origin response without the header, and the token must
      // not ship in client JavaScript.
      "/pythpro": {
        target: "https://pyth.dourolabs.app",
        changeOrigin: true,
        rewrite: (p) => p.replace(/^\/pythpro/, "/v1"),
        headers: { Authorization: `Bearer ${hermesToken()}` },
      },
    },
  },
});
