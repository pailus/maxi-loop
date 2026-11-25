import type { ReactNode } from "react";
import { Cpu, Globe, Monitor, Package, Server, Smartphone } from "lucide-react";

/** Sentry-style event context (F029): browser, OS, runtime, device, server, SDK and request. */

type Json = Record<string, unknown>;

interface ContextCard {
  key: string;
  label: string;
  icon: ReactNode;
  name: string;
  detail?: string;
}

interface RequestInfo {
  method?: string;
  url?: string;
  queryString?: string;
  statusCode?: string;
  headers: Array<[string, string]>;
}

/** Contexts rendered as cards or elsewhere; everything else is listed under "Other contexts". */
const HANDLED_CONTEXTS = new Set([
  "browser",
  "os",
  "client_os",
  "runtime",
  "device",
  "response",
  "trace",
  "profile",
]);

function asObject(value: unknown): Json | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Json)
    : undefined;
}

function asText(value: unknown): string | undefined {
  if (typeof value === "string") return value.trim() || undefined;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return undefined;
}

function nameVersion(ctx: Json | undefined): { name: string; version?: string } | undefined {
  const name = asText(ctx?.name);
  return name ? { name, version: asText(ctx?.version) } : undefined;
}

function headerPairs(raw: unknown): Array<[string, string]> {
  if (Array.isArray(raw)) {
    return raw
      .filter((p): p is unknown[] => Array.isArray(p) && p.length >= 2)
      .map((p) => [String(p[0]), asText(p[1]) ?? ""]);
  }
  const obj = asObject(raw);
  return obj ? Object.entries(obj).map(([k, v]) => [k, asText(v) ?? JSON.stringify(v)]) : [];
}

function buildCards(payload: Json, contexts: Json): ContextCard[] {
  const cards: ContextCard[] = [];
  const push = (
    key: string,
    label: string,
    icon: ReactNode,
    nv: { name: string; version?: string } | undefined,
  ) => {
    if (nv) cards.push({ key, label, icon, name: nv.name, detail: nv.version });
  };

  push("browser", "Browser", <Globe size={14} />, nameVersion(asObject(contexts.browser)));
  push("client_os", "Client OS", <Monitor size={14} />, nameVersion(asObject(contexts.client_os)));
  push("os", "Operating system", <Monitor size={14} />, nameVersion(asObject(contexts.os)));
  push("runtime", "Runtime", <Cpu size={14} />, nameVersion(asObject(contexts.runtime)));

  const device = asObject(contexts.device);
  const deviceName =
    asText(device?.model) ?? asText(device?.family) ?? asText(device?.name);
  if (deviceName) {
    cards.push({
      key: "device",
      label: "Device",
      icon: <Smartphone size={14} />,
      name: deviceName,
      detail: asText(device?.arch),
    });
  }

  const serverName = asText(payload.server_name);
  if (serverName) {
    cards.push({ key: "server", label: "Server", icon: <Server size={14} />, name: serverName });
  }
  push("sdk", "SDK", <Package size={14} />, nameVersion(asObject(payload.sdk)));
  return cards;
}

function buildRequest(payload: Json, contexts: Json): RequestInfo | undefined {
  const req = asObject(payload.request);
  const statusCode = asText(asObject(contexts.response)?.status_code);
  if (!req && !statusCode) return undefined;
  const info: RequestInfo = {
    method: asText(req?.method),
    url: asText(req?.url),
    queryString: asText(req?.query_string),
    statusCode,
    headers: headerPairs(req?.headers),
  };
  return info.method || info.url || info.statusCode || info.headers.length > 0
    ? info
    : undefined;
}

function otherContexts(contexts: Json): Array<[string, Array<[string, string]>]> {
  return Object.entries(contexts)
    .filter(([key]) => !HANDLED_CONTEXTS.has(key))
    .map(([key, value]): [string, Array<[string, string]>] => [
      key,
      Object.entries(asObject(value) ?? {})
        .filter(([field]) => field !== "type")
        .map(([field, v]) => [field, asText(v) ?? JSON.stringify(v)]),
    ])
    .filter(([, fields]) => fields.length > 0);
}

interface EventContextViewProps {
  payload: unknown;
  /** Drawer mode: cards + request line only. */
  compact?: boolean;
}

export function EventContextView({ payload, compact = false }: EventContextViewProps) {
  const root = asObject(payload) ?? {};
  const contexts = asObject(root.contexts) ?? {};
  const cards = buildCards(root, contexts);
  const request = buildRequest(root, contexts);
  const others = compact ? [] : otherContexts(contexts);

  if (cards.length === 0 && !request && others.length === 0) {
    return (
      <p className="text-xs text-[var(--color-text-dim)]">
        No browser, OS, runtime or request context in the sample event.
      </p>
    );
  }

  return (
    <div className="space-y-3">
      {cards.length > 0 && (
        <div className={`grid gap-2 ${compact ? "grid-cols-2" : "grid-cols-2 md:grid-cols-3"}`}>
          {cards.map((card) => (
            <div
              key={card.key}
              className="rounded-md border border-[var(--color-border)] px-2.5 py-2 min-w-0"
            >
              <div className="flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wide text-[var(--color-text-dim)]">
                {card.icon}
                <span>{card.label}</span>
              </div>
              <div className="mt-1 text-xs font-semibold text-[var(--color-text)] truncate" title={card.name}>
                {card.name}
              </div>
              {card.detail && (
                <div className="pm-mono text-[11px] text-[var(--color-text-muted)] truncate" title={card.detail}>
                  {card.detail}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {request && (
        <div className="rounded-md border border-[var(--color-border)] overflow-hidden">
          <div className="px-2.5 py-2 flex items-center gap-2 bg-[var(--color-bg-muted)] text-xs min-w-0">
            {request.method && <span className="pm-lozenge pm-mono">{request.method}</span>}
            <span className="pm-mono text-[var(--color-text)] truncate" title={request.url}>
              {request.url ?? "—"}
            </span>
            {request.statusCode && (
              <span className="ml-auto pm-lozenge pm-mono shrink-0">{request.statusCode}</span>
            )}
          </div>
          {!compact && (request.queryString || request.headers.length > 0) && (
            <dl className="grid grid-cols-[minmax(110px,180px)_1fr] gap-x-3 gap-y-1 px-2.5 py-2 text-[11px]">
              {request.queryString && (
                <>
                  <dt className="font-semibold text-[var(--color-text-dim)]">Query string</dt>
                  <dd className="pm-mono text-[var(--color-text)] break-all">{request.queryString}</dd>
                </>
              )}
              {request.headers.map(([k, v]) => (
                <div key={k} className="contents">
                  <dt className="font-semibold text-[var(--color-text-dim)] break-all">{k}</dt>
                  <dd className="pm-mono text-[var(--color-text)] break-all">{v}</dd>
                </div>
              ))}
            </dl>
          )}
        </div>
      )}

      {others.map(([name, fields]) => (
        <div key={name} className="rounded-md border border-[var(--color-border)] px-2.5 py-2">
          <div className="text-[10px] font-semibold uppercase tracking-wide text-[var(--color-text-dim)] mb-1">
            {name}
          </div>
          <dl className="grid grid-cols-[minmax(110px,180px)_1fr] gap-x-3 gap-y-1 text-[11px]">
            {fields.map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="font-semibold text-[var(--color-text-dim)]">{k}</dt>
                <dd className="pm-mono text-[var(--color-text)] break-all">{v}</dd>
              </div>
            ))}
          </dl>
        </div>
      ))}
    </div>
  );
}
