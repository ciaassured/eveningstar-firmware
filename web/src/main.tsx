import { render } from "preact";
import { App } from "./app/App.tsx";
import { createServices } from "./app/services.ts";
import "./styles/tokens.css";
import "./styles/base.css";

const INPUT_EVENTS = ["pointerdown", "keydown", "wheel", "touchstart"] as const;

function unloadGuard(event: BeforeUnloadEvent): void {
  event.preventDefault();
}

/** Saves bytes through a link with a `download` name, as the page fetches downloads itself. */
function save(bytes: Uint8Array, filename: string): void {
  const url = URL.createObjectURL(new Blob([bytes as Uint8Array<ArrayBuffer>], { type: "application/octet-stream" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.hidden = true;
  document.body.append(link);
  link.click();
  link.remove();
  // The browser reads the URL after the click returns, so it is revoked later.
  globalThis.setTimeout(() => {
    URL.revokeObjectURL(url);
  }, 30_000);
}

async function sha256(bytes: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as Uint8Array<ArrayBuffer>));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

// Hashing and the clipboard exist only in a secure context; over plain HTTP the page shows what to check by hand.
const secure = globalThis.isSecureContext;

const services = createServices({
  downloads: {
    save,
    sha256: secure ? sha256 : undefined,
    guardUnload(on) {
      if (on) globalThis.addEventListener("beforeunload", unloadGuard);
      else globalThis.removeEventListener("beforeunload", unloadGuard);
    },
  },
  clipboard: secure ? (text) => navigator.clipboard.writeText(text) : undefined,
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
