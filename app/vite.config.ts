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
  // Vite pre-bundles dependencies; the SDK is source we want it to watch, not pre-bundle.
  optimizeDeps: { exclude: ["@solfx/client"] },
  server: {
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
    },
  },
});
