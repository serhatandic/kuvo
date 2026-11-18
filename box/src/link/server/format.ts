import { UnsupportedFormatVersion } from "../../shared/format-version";
import { ROUTE_VERSION, ROUTE_VERSION_HEADER, routeVersionError } from "../../shared/route-version";
import { isObject } from "../../shared/json";
import type { LinkHandler, LinkRequest } from "./context";

export const routeVersionHeaders = { [ROUTE_VERSION_HEADER]: ROUTE_VERSION };

/** Relay and SSH carry headers in JSON, so unlike HTTP they can have differently cased keys. */
export function routeRequestHeaders(given: unknown) {
  const headers: LinkRequest["headers"] = Object.create(null);
  for (const [name, value] of Object.entries(isObject(given) ? given : {})) {
    const key = name.toLowerCase();
    const invalidVersion = key === ROUTE_VERSION_HEADER && routeVersionError(value);
    if (invalidVersion) return { error: invalidVersion };
    if (value === undefined) continue;
    if (typeof value !== "string" && !(Array.isArray(value) && value.every(each => typeof each === "string"))) {
      return { error: { code: "invalidHeader", message: `Invalid request header: ${key}.` } };
    }
    if (Object.hasOwn(headers, key) && JSON.stringify(headers[key]) !== JSON.stringify(value)) {
      return { error: { code: "conflictingHeader", message: `Conflicting request header: ${key}.` } };
    }
    headers[key] = value;
  }
  return { headers };
}

export const routeRefusalHeaders = { ...routeVersionHeaders, "content-type": "application/json" };

/** HTTP, relay requests and SSH all use the same route body contract. Absent means the
 * original contract. Refuse other schemas before any route acts on a request. */
export function versionedRoutes(handle: LinkHandler): LinkHandler {
  return async function* (request, phone, signal) {
    const { headers, error } = routeRequestHeaders(request.headers);
    if (error) {
      yield { head: { status: 400, headers: routeRefusalHeaders } };
      yield { body: Buffer.from(JSON.stringify(error)) };
      return;
    }
    let started = false;
    try {
      for await (const piece of handle({ ...request, headers }, phone, signal)) {
        started = true;
        yield "head" in piece ? { head: { ...piece.head, headers: { ...piece.head.headers, ...routeVersionHeaders } } } : piece;
      }
    } catch (error) {
      if (started || !(error instanceof UnsupportedFormatVersion)) throw error;
      yield { head: { status: 409, headers: routeRefusalHeaders } };
      yield { body: Buffer.from(JSON.stringify({ code: "unsupportedFormatVersion", message: error.message })) };
    }
  };
}
