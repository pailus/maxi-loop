import { useEffect, useState } from "react";

/**
 * Query-string UI state (D014): menu (`view`), tabs (`tab`, `status`, `detail`),
 * selection (`project`, `workspace`, `issue`, `full`, `trace`) live in the URL so a
 * refresh or shared link reopens the same screen. Values read from the URL are
 * untrusted: callers validate them (enum allow-lists, numeric ids).
 */

export function getUrlParam(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

/** Set (string) or remove (null/undefined/"") query params. `push` adds a history entry. */
export function setUrlParams(
  updates: Record<string, string | null | undefined>,
  mode: "push" | "replace" = "replace",
): void {
  const params = new URLSearchParams(window.location.search);
  for (const [key, value] of Object.entries(updates)) {
    if (value === null || value === undefined || value === "") params.delete(key);
    else params.set(key, value);
  }
  const search = params.toString();
  const next = `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`;
  const current = `${window.location.pathname}${window.location.search}${window.location.hash}`;
  if (next === current) return;
  if (mode === "push") window.history.pushState(null, "", next);
  else window.history.replaceState(null, "", next);
}

function readEnum<T extends string>(param: string, allowed: readonly T[], fallback: T): T {
  const raw = getUrlParam(param);
  return raw !== null && (allowed as readonly string[]).includes(raw) ? (raw as T) : fallback;
}

/**
 * `useState` for a tab/enum mirrored in the query string. The fallback value is
 * kept out of the URL; back/forward (`popstate`) re-reads the param.
 */
export function useUrlEnum<T extends string>(
  param: string,
  allowed: readonly T[],
  fallback: T,
  initialOverride?: T,
): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() =>
    getUrlParam(param) !== null
      ? readEnum(param, allowed, fallback)
      : (initialOverride ?? fallback),
  );

  useEffect(() => {
    setUrlParams({ [param]: value === fallback ? null : value });
  }, [param, value, fallback]);

  useEffect(() => {
    const onPop = () => setValue(readEnum(param, allowed, fallback));
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
    // `allowed` is a module-level constant at every call site.
  }, [param, fallback]);

  return [value, setValue];
}
