import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite-plus";

const apiTarget = process.env.AUDIOROUTER_API ?? "http://127.0.0.1:7822";

export default defineConfig({
  resolve: { tsconfigPaths: true },
  plugins: [react(), tailwindcss()],
  build: {
    outDir: process.env.AUDIOROUTER_DIST_DIR ?? "dist",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/api": {
        target: apiTarget,
        changeOrigin: true,
      },
    },
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
