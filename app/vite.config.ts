import { fileURLToPath, URL } from "node:url";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

const sdk = fileURLToPath(new URL("../clients/js/src", import.meta.url));

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
  server: { fs: { allow: [fileURLToPath(new URL("..", import.meta.url))] } },
});
