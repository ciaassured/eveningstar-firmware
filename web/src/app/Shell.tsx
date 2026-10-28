import { signal } from "@preact/signals";
import type { ComponentChildren } from "preact";
import { Banner } from "../ui/Banner.tsx";
import { Button } from "../ui/Button.tsx";
import { Mark, Wordmark } from "../ui/Brand.tsx";
import { ConnectionIndicator } from "../ui/ConnectionIndicator.tsx";
import { useServices } from "./context.ts";
import { NAVIGATION, pathOf } from "./router.ts";
import styles from "./shell.module.css";

/** How many restarts of the device the person has dismissed the notice of. */
const dismissedRestarts = signal(0);

function Notices() {
  const { activity, device } = useServices();
  return (
    <>
      {activity.idle.value ? (
        <Banner level="standby" title="Paused to save device connections">
          <p>Live values stopped updating because the page hasn't been used for a while.</p>
          <div>
            <Button variant="primary" onClick={() => activity.resume()}>
              Resume
            </Button>
          </div>
        </Banner>
      ) : null}
      {device.restarts.value > dismissedRestarts.value ? (
        <Banner level="caution" title="The device restarted">
          <p>Its uptime started again, so the values on this page were fetched again.</p>
          <div>
            <Button
              onClick={() => {
                dismissedRestarts.value = device.restarts.value;
              }}
            >
              Dismiss
            </Button>
          </div>
        </Banner>
      ) : null}
    </>
  );
}

/** The status bar, the navigation and the room for the page. */
export function Shell({ children }: { children: ComponentChildren }) {
  const { connection, device, router } = useServices();
  const route = router.route.value;
  const name = device.system.value.value?.name;

  return (
    <div class={styles["shell"]}>
      <header class={styles["bar"]}>
        <a class={styles["brand"]} href="#/">
          <Mark />
          <Wordmark />
        </a>
        <span class={styles["device"]}>{name ?? ""}</span>
        <div class={styles["end"]}>
          <ConnectionIndicator state={connection.state.value} />
          <span class={styles["badge"]}>Operational</span>
        </div>
      </header>
      <nav class={styles["nav"]} aria-label="Main">
        <ul>
          {NAVIGATION.map((item) => (
            <li key={item.route}>
              <a
                class={styles["link"]}
                href={`#${item.path}`}
                aria-current={route.name === item.route ? "page" : undefined}
              >
                {item.label}
              </a>
            </li>
          ))}
        </ul>
      </nav>
      <main class={styles["main"]} id="main" data-route={pathOf(route)}>
        <Notices />
        {children}
      </main>
    </div>
  );
}
