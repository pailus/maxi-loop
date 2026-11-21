export interface HealthPayload {
  status: string;
  service: string;
  version: string;
  database: string;
}

export async function fetchHealth(): Promise<HealthPayload> {
  const res = await fetch("/api/v1/health");
  return (await res.json()) as HealthPayload;
}
