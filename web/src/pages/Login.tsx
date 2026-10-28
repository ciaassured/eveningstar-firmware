import { useServices } from "../app/context.ts";
import { Banner } from "../ui/Banner.tsx";
import { Button } from "../ui/Button.tsx";
import styles from "./pages.module.css";

/** The place the person is sent when the device asks them to sign in. */
export function Login() {
  const { router } = useServices();
  return (
    <>
      <h1 class={styles["heading"]}>Sign in</h1>
      <Banner level="standby" title="Signing in isn't available yet">
        <p>This device doesn't have authentication yet, so there is nothing to sign in to.</p>
        <div>
          <Button
            onClick={() => {
              router.returnFromLogin();
            }}
          >
            Go back
          </Button>
        </div>
      </Banner>
    </>
  );
}
