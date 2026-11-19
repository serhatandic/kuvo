import { UnsupportedFormatVersion } from "./format-version";

export const ROUTE_VERSION_HEADER = "x-kuvo-route-version";
export const ROUTE_VERSION = "1";

export function routeVersionError(given: unknown) {
  if (given === undefined || given === ROUTE_VERSION) return null;
  const version = typeof given === "string" && /^\d{1,15}$/.test(given) ? Number(given) : undefined;
  const error = new UnsupportedFormatVersion("Kuvo route", version, Number(ROUTE_VERSION));
  return { code: "unsupportedFormatVersion", message: error.message };
}
