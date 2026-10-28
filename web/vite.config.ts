import { defineConfig } from "vite";

export default defineConfig({
  base: "/",
  esbuild: {
    jsx: "automatic",
    jsxImportSource: "preact",
  },
  server: { host: "127.0.0.1" },
  build: {
    modulePreload: false,
    cssCodeSplit: false,
    assetsInlineLimit: 100_000_000,
    target: "es2022",
    sourcemap: false,
  },
});
