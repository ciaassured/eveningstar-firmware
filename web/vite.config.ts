import { defineConfig } from "vite";

// The device whose API the dev server forwards `/api` to, such as `http://192.168.1.50`.
const device = process.env["EVENINGSTAR_DEVICE"];

export default defineConfig({
  base: "/",
  // The packages are read-only, so the dev server's cache lives beside the sources.
  cacheDir: ".vite",
  oxc: {
    jsx: { runtime: "automatic", importSource: "preact" },
  },
  server: {
    host: "127.0.0.1",
    ...(device === undefined ? {} : { proxy: { "/api": { target: device, changeOrigin: true } } }),
  },
  build: {
    modulePreload: false,
    cssCodeSplit: false,
    assetsInlineLimit: 100_000_000,
    target: "es2022",
    sourcemap: false,
  },
});
