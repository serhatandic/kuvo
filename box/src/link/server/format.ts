import { ROUTE_VERSION, ROUTE_VERSION_HEADER, routeVersionError } from "../../shared/route-version";
import type { LinkHandler } from "./context";

/** HTTP, relay requests and SSH all use the same route body contract. Absent means the
 * original contract. Refuse other schemas before any route acts on a request. */
export function versionedRoutes(handle: LinkHandler): LinkHandler {
  return async function* (request, phone, signal) {
    const error = routeVersionError(request.headers[ROUTE_VERSION_HEADER]);
    const headers = { [ROUTE_VERSION_HEADER]: ROUTE_VERSION };
    if (error) {
      yield { head: { status: 400, headers: { ...headers, "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify(error)) };
      return;
    }
    for await (const piece of handle(request, phone, signal)) {
      yield "head" in piece ? { head: { ...piece.head, headers: { ...piece.head.headers, ...headers } } } : piece;
    }
  };
}
