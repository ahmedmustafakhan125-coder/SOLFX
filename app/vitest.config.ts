/**
 * Vitest resolves the `@/` alias the same way Vite does for the app itself. Kept as its own
 * config rather than folded into `vite.config.ts` so the dev-server proxies — which exist
 * only to keep the Pyth key server-side — are not loaded by a test run.
 */
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: [
      // Same as vite.config.ts: the SDK is consumed as TypeScript source. Missing here, any test
      // importing `@solfx/client` failed to load — which went unnoticed only because no app test
      // did until the NOXFUNDS marketplace.
      {
        find: /^@solfx\/client$/,
        replacement: fileURLToPath(
          new URL("../clients/js/src/index.ts", import.meta.url)
        ),
      },
      {
        find: "@",
        replacement: fileURLToPath(new URL("./src", import.meta.url)),
      },
    ],
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
