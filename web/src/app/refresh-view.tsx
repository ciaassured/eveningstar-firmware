import { PAGE } from "../storage/copy.ts";
import { RefreshButton } from "../ui/RefreshButton.tsx";
import { useServices } from "./context.ts";

/** The header's Refresh button of the storage page: it reloads the storage view. */
export function StorageRefreshButton() {
  const { storage } = useServices();
  return (
    <RefreshButton
      label={PAGE.refresh}
      busyLabel={PAGE.refreshing}
      refreshing={storage.refreshing.value}
      onClick={() => {
        void storage.refresh("action");
      }}
    />
  );
}
