import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite-plus";
import { apiServer } from "./plugins/api-server";
import { prerenderBundle } from "./plugins/prerender-bundle";

const dashboardRoot = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  root: dashboardRoot,
  resolve: { tsconfigPaths: true },
  plugins: [
    react(),
    tailwindcss(),
    // Dev: spawns audiorouter-dashboard-api (Rust) + configures /api proxy.
    // Build: no-op.
    apiServer(),
    // Build-time SSG: renders <App /> to static HTML via the required
    // `prerender` export entry, then cleans up prerender artifacts.
    // In dev, this is a no-op.
    ...prerenderBundle({ prerenderScript: "src/main.tsx" }),
  ],
  build: {
    outDir: process.env.AUDIOROUTER_DIST_DIR ?? "dist",
    emptyOutDir: true,
  },
  lint: {
    options: {
      typeAware: true,
      typeCheck: true,
    },
  },
  fmt: {
    sortImports: {
      newlinesBetween: false,
    },
  },
});
