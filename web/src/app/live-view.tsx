import { describeFailure } from "../api/failure.ts";
import { rebootHolderLabel, rebootReasonLabel } from "../device/labels.ts";
import type { RebootStatus } from "../device/types.ts";
import { formatAge, formatCountdown } from "../lib/format.ts";
import { freshness } from "../live/freshness.ts";
import type { LiveSource } from "../live/poller.ts";
import { Banner } from "../ui/Banner.tsx";
import { Age } from "../ui/Panel.tsx";
import { useServices } from "./context.ts";

/** How old a live value is, in words, and whether that's too old to call current. */
export function useAge(source: LiveSource<unknown>): { text: string; stale: boolean } | undefined {
  const { now } = useServices();
  const current = now.value;
  const fresh = freshness(source.updatedAt.value, current, source.intervalMs);
  return fresh === undefined ? undefined : { text: formatAge(fresh.ageMs), stale: fresh.stale };
}

/** A live value's age, as the line under what it shows. */
export function LiveAge({ source }: { source: LiveSource<unknown> }) {
  const age = useAge(source);
  return age === undefined ? null : <Age text={age.text} stale={age.stale} />;
}

/** Why a live value couldn't be refreshed, if it couldn't; the old value stays on the page, marked by its age. */
export function LiveError({ source }: { source: LiveSource<unknown> }) {
  const failure = source.error.value;
  if (failure === undefined) return null;
  const message = describeFailure(failure);
  return (
    <Banner level="caution" title={`Can't refresh: ${message.title}`}>
      <p>{message.explanation}</p>
    </Banner>
  );
}

/** A graceful reboot that is waiting for holds to be released: why, how long it has, and what holds it. */
export function PendingReboot({ status, fetchedAt }: { status: RebootStatus; fetchedAt: number | undefined }) {
  const { now } = useServices();
  const { pending } = status;
  if (pending === null) return null;
  const elapsedS = fetchedAt === undefined ? 0 : Math.max(0, (now.value - fetchedAt) / 1_000);
  return (
    <Banner level="caution" title="Reboot pending">
      <p>{rebootReasonLabel(pending.reason)}.</p>
      <p>
        The device restarts in {formatCountdown(pending.remaining_s - elapsedS)}, or sooner once nothing holds it off.
      </p>
      {status.holds.length > 0 ? (
        <ul>
          {status.holds.map((hold) => (
            <li key={hold.holder}>
              {rebootHolderLabel(hold.holder)}: {hold.count}
            </li>
          ))}
        </ul>
      ) : null}
    </Banner>
  );
}
