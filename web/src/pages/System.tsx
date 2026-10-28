import { useState } from "preact/hooks";
import { useServices } from "../app/context.ts";
import { FailureView } from "../app/FailureView.tsx";
import { LoadGate } from "../app/LoadGate.tsx";
import { LiveAge, LiveError, PendingReboot, useAge } from "../app/live-view.tsx";
import { Requires } from "../app/Requires.tsx";
import { rebootModeLabel } from "../device/labels.ts";
import type { RebootMode } from "../device/types.ts";
import { formatUptime } from "../lib/format.ts";
import { Banner } from "../ui/Banner.tsx";
import { Actions, Button } from "../ui/Button.tsx";
import { Dialog } from "../ui/Dialog.tsx";
import { Metric, Panel, PanelGrid } from "../ui/Panel.tsx";
import styles from "./pages.module.css";

const GROUP = "system";
const MODES: readonly RebootMode[] = ["graceful", "immediate"];

/** Where a reboot the person asked for has got to. */
function RebootProgress() {
  const { device } = useServices();
  const progress = device.reboot.progress.value;
  const done = (): void => {
    device.reboot.reset();
    void device.system.load("action");
    void device.rebootStatus.load("action");
  };

  switch (progress.kind) {
    case "idle":
      return null;
    case "requesting":
      return <p class={styles["progress"]}>Sending the request…</p>;
    case "waiting":
      return progress.status?.pending ? (
        <PendingReboot status={progress.status} fetchedAt={progress.fetchedAt} />
      ) : (
        <p class={styles["progress"]}>Waiting for the device to restart…</p>
      );
    case "restarting":
      return (
        <Banner level="caution" title="Restarting">
          <p>The device isn't answering while it restarts. This page keeps asking.</p>
        </Banner>
      );
    case "restarted":
      return (
        <Banner level="normal" title="Restarted">
          <p>The device is back, with its uptime starting again.</p>
          <div>
            <Button variant="primary" onClick={done}>
              Done
            </Button>
          </div>
        </Banner>
      );
    case "timed-out":
      return (
        <Banner level="serious" title="The device hasn't come back after 2 minutes" urgent>
          <p>
            It may still be starting, its network may have changed, or something may have stopped it from starting.
            Check its power and network cable, then reload the page.
          </p>
          <div>
            <Button onClick={done}>Close</Button>
          </div>
        </Banner>
      );
    case "failed":
      return (
        <>
          <FailureView failure={progress.failure} context={progress.context} />
          <div>
            <Button onClick={done}>Close</Button>
          </div>
        </>
      );
  }
}

function RebootPanel() {
  const { device } = useServices();
  const [confirming, setConfirming] = useState(false);
  const [mode, setMode] = useState<RebootMode>("graceful");
  const busy = device.reboot.progress.value.kind !== "idle";

  const confirm = (): void => {
    setConfirming(false);
    void device.reboot.start(mode, device.system.value.value?.uptime_s ?? 0);
  };

  return (
    <Panel title="Reboot" wide>
      <p>Restarting the device interrupts what it is doing and drops its connections for a short while.</p>
      <Requires capability="operate" mode="disable">
        <Button
          disabled={busy}
          reason={busy ? "A reboot is in progress" : undefined}
          onClick={() => {
            setConfirming(true);
          }}
        >
          Reboot…
        </Button>
      </Requires>
      <RebootProgress />
      <Dialog
        open={confirming}
        title="Reboot the device?"
        onClose={() => {
          setConfirming(false);
        }}
      >
        <fieldset class={styles["radios"]}>
          <legend>How</legend>
          {MODES.map((option) => (
            <label key={option} class={styles["radio"]}>
              <input
                type="radio"
                name="reboot-mode"
                value={option}
                checked={mode === option}
                onChange={() => {
                  setMode(option);
                }}
              />
              {rebootModeLabel(option)}
            </label>
          ))}
        </fieldset>
        <p>
          A graceful reboot waits for work in progress to finish, up to a deadline. An immediate reboot doesn't wait.
        </p>
        <Actions>
          <Button variant="primary" onClick={confirm}>
            Reboot
          </Button>
          <Button
            onClick={() => {
              setConfirming(false);
            }}
          >
            Cancel
          </Button>
        </Actions>
      </Dialog>
    </Panel>
  );
}

function Content() {
  const { device } = useServices();
  const system = device.system.value.value;
  const age = useAge(device.system);
  const stale = age?.stale ?? false;

  return (
    <>
      <h1 class={styles["heading"]}>System</h1>
      <LiveError source={device.system} />
      <PanelGrid>
        <Panel title="Identity">
          <Metric label="Name" value={system?.name ?? "–"} mono />
          <Metric label="ID" value={system?.id ?? "–"} mono />
        </Panel>
        <Panel title="Uptime" stale={stale}>
          <Metric label="Since the device started" value={system === undefined ? "–" : formatUptime(system.uptime_s)} />
          <LiveAge source={device.system} />
        </Panel>
        <RebootPanel />
      </PanelGrid>
    </>
  );
}

export function System() {
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
