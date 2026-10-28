import { Banner } from "../ui/Banner.tsx";
import styles from "./pages.module.css";

export function NotFound({ path }: { path: string }) {
  return (
    <>
      <h1 class={styles["heading"]}>Page not found</h1>
      <Banner level="caution" title="There is no page at this address">
        <p>
          <code>{path}</code> isn't a page of the web UI. <a href="#/">Go to the dashboard.</a>
        </p>
      </Banner>
    </>
  );
}
