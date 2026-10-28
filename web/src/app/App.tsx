import { Dashboard } from "../pages/Dashboard.tsx";
import { Login } from "../pages/Login.tsx";
import { NotFound } from "../pages/NotFound.tsx";
import { System } from "../pages/System.tsx";
import { ServicesProvider, useServices } from "./context.ts";
import { Shell } from "./Shell.tsx";
import type { Services } from "./services.ts";

function RouteView() {
  const { router } = useServices();
  const route = router.route.value;
  switch (route.name) {
    case "dashboard":
      return <Dashboard />;
    case "system":
      return <System />;
    case "login":
      return <Login />;
    case "not-found":
      return <NotFound path={route.path} />;
  }
}

export function App({ services }: { services: Services }) {
  return (
    <ServicesProvider value={services}>
      <Shell>
        <RouteView />
      </Shell>
    </ServicesProvider>
  );
}
