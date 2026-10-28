import { createContext } from "preact";
import { useContext } from "preact/hooks";
import type { Services } from "./services.ts";

const ServicesContext = createContext<Services | undefined>(undefined);

export const ServicesProvider = ServicesContext.Provider;

/** The services the page shares. */
export function useServices(): Services {
  const services = useContext(ServicesContext);
  if (services === undefined) throw new Error("no services are provided");
  return services;
}
