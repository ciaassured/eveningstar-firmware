import { useEffect, useState } from "preact/hooks";

/** Whether the media query matches, following it as the window changes. */
export function useMedia(query: string): boolean {
  const [matches, setMatches] = useState(() => globalThis.matchMedia(query).matches);
  useEffect(() => {
    const list = globalThis.matchMedia(query);
    const change = (): void => {
      setMatches(list.matches);
    };
    change();
    list.addEventListener("change", change);
    return () => {
      list.removeEventListener("change", change);
    };
  }, [query]);
  return matches;
}
