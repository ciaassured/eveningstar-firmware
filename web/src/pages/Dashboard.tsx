import { useServices } from "../app/context.ts";
import { LoadGate } from "../app/LoadGate.tsx";
import { LiveAge, LiveError, PendingReboot, useAge } from "../app/live-view.tsx";
import { formatUptime } from "../lib/format.ts";
import { Metric, Panel, PanelGrid } from "../ui/Panel.tsx";
import styles from "./pages.module.css";

const GROUP = "dashboard";

function Content() {
  const { device } = useServices();
  const system = device.system.value.value;
  const reboot = device.rebootStatus.value.value;
  const age = useAge(device.system);

  return (
    <>
      <h1 class={styles["heading"]}>Dashboard</h1>
      <LiveError source={device.system} />
      {reboot === undefined ? null : (
        <PendingReboot status={reboot} fetchedAt={device.rebootStatus.updatedAt.value} />
      )}
      <PanelGrid>
        <Panel title="Uptime" stale={age?.stale ?? false}>
          <Metric label="Since the device started" value={system === undefined ? "–" : formatUptime(system.uptime_s)} />
          <LiveAge source={device.system} />
        </Panel>
      </PanelGrid>
    </>
  );
}

export function Dashboard() {
  const { device } = useServices();
  return (
    <LoadGate
      group={GROUP}
      loaders={[() => device.system.load("view", GROUP), () => device.rebootStatus.load("view", GROUP)]}
    >
      <Content />
    </LoadGate>
  );
}
