import { render } from "preact";
import { App } from "./app/App.tsx";
import { createServices } from "./app/services.ts";
import "./styles/tokens.css";
import "./styles/base.css";

const INPUT_EVENTS = ["pointerdown", "keydown", "wheel", "touchstart"] as const;

const services = createServices({
  fetch: (url, init) => fetch(url, init),
  timers: {
    setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
    clearTimeout: (handle) => {
      globalThis.clearTimeout(handle as number | undefined);
    },
  },
  random: () => Math.random(),
  now: () => Date.now(),
  activity: {
    visible: () => document.visibilityState === "visible",
    onVisibilityChange(listener) {
      document.addEventListener("visibilitychange", listener);
      return () => {
        document.removeEventListener("visibilitychange", listener);
      };
    },
    onInput(listener) {
      for (const type of INPUT_EVENTS) document.addEventListener(type, listener, { passive: true });
      return () => {
        for (const type of INPUT_EVENTS) document.removeEventListener(type, listener);
      };
    },
    timers: {
      setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
      clearTimeout: (handle) => {
        globalThis.clearTimeout(handle as number | undefined);
      },
    },
  },
  router: {
    hash: () => globalThis.location.hash,
    setHash(hash) {
      globalThis.location.hash = hash;
    },
    onHashChange(listener) {
      globalThis.addEventListener("hashchange", listener);
      return () => {
        globalThis.removeEventListener("hashchange", listener);
      };
    },
  },
});

const root = document.getElementById("app");
if (root !== null) {
  render(<App services={services} />, root);
  services.device.start();
}
