import { UnsupportedFormatVersion } from "./format-version";

export const ROUTE_VERSION_HEADER = "x-kuvo-route-version";
export const ROUTE_VERSION = "1";

/** A direct local consumer must refuse before interpreting a reply as a receipt. */
export async function checkRouteReply(response: Response): Promise<void> {
  const refusal = routeVersionError(response.headers.get(ROUTE_VERSION_HEADER) ?? undefined);
  if (!refusal) return;
  await response.body?.cancel().catch(() => {});
  throw new Error(refusal.message);
}

export function routeVersionError(given: unknown) {
  if (given === undefined || given === ROUTE_VERSION) return null;
  const version = typeof given === "string" && /^\d{1,15}$/.test(given) ? Number(given) : undefined;
  const error = new UnsupportedFormatVersion("Kuvo route", version, Number(ROUTE_VERSION));
  return { code: "unsupportedFormatVersion", message: error.message };
}
