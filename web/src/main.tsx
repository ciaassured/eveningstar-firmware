import { signal } from "@preact/signals";
import { render } from "preact";
import styles from "./app.module.css";
import { formatUptime } from "./lib/format.ts";

const uptime = signal(3_660);

function App() {
  return (
    <div class={styles["app"]}>
      <p>Uptime {formatUptime(uptime.value)}</p>
    </div>
  );
}

const root = document.getElementById("app");
if (root) {
  render(<App />, root);
}
