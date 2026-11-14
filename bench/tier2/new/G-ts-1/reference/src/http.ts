// The HTTP surface: a function from a web Request to a web Response.

import { ConflictError, NoRoomError, NotFoundError, ValidationError } from "./errors.ts";
import type { BookingRequest, Scheduler, SlotRequest } from "./scheduler.ts";

type Handler = (request: Request) => Promise<Response>;
type Route = { method: string; pattern: RegExp; run: (match: string[], url: URL, request: Request) => Promise<Response> };

class BadRequest extends Error {}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

async function readObject(request: Request): Promise<Record<string, unknown>> {
  let body: unknown;
  try {
    body = JSON.parse(await request.text());
  } catch {
    throw new BadRequest("body is not valid JSON");
  }
  if (body === null || typeof body !== "object" || Array.isArray(body)) {
    throw new BadRequest("body must be a JSON object");
  }
  return body as Record<string, unknown>;
}

function required(url: URL, name: string): string {
  const value = url.searchParams.get(name);
  if (value === null || value === "") throw new BadRequest(`query value ${name} is required`);
  return value;
}

function integer(url: URL, name: string, optional = false): number | undefined {
  const value = url.searchParams.get(name);
  if (value === null && optional) return undefined;
  if (value === null || !/^\d+$/.test(value)) throw new BadRequest(`query value ${name} must be digits`);
  return Number(value);
}

function errorResponse(err: unknown): Response {
  if (err instanceof BadRequest || err instanceof ValidationError) return json(400, { error: err.message });
  if (err instanceof NotFoundError) return json(404, { error: err.message });
  if (err instanceof ConflictError) return json(409, { error: err.message, conflicts: err.conflicts });
  if (err instanceof NoRoomError) return json(422, { error: err.message });
  throw err;
}

export function createHandler(scheduler: Scheduler): Handler {
  const routes: Route[] = [
    { method: "POST", pattern: /^\/rooms$/, run: async (_m, _u, req) => {
      const body = await readObject(req);
      return json(201, scheduler.addRoom(body as { id: string; capacity: number; features?: string[] }));
    } },
    { method: "GET", pattern: /^\/rooms$/, run: async () => json(200, scheduler.rooms()) },
    { method: "POST", pattern: /^\/bookings$/, run: async (_m, _u, req) => {
      const body = await readObject(req);
      return json(201, scheduler.book(body as unknown as BookingRequest));
    } },
    { method: "GET", pattern: /^\/bookings\/([^/]+)$/, run: async (m) => json(200, scheduler.get(m[1])) },
    { method: "DELETE", pattern: /^\/bookings\/([^/]+)$/, run: async (m) => {
      scheduler.cancel(m[1]);
      return new Response(null, { status: 204 });
    } },
    { method: "GET", pattern: /^\/rooms\/([^/]+)\/bookings$/, run: async (m, url) =>
      json(200, scheduler.bookings(m[1], required(url, "from"), required(url, "to"))) },
    { method: "GET", pattern: /^\/rooms\/([^/]+)\/free$/, run: async (m, url) => {
      const min = integer(url, "min", true);
      return json(200, scheduler.freeSlots(m[1], required(url, "from"), required(url, "to"), min ?? 1));
    } },
    { method: "GET", pattern: /^\/slots$/, run: async (_m, url) => {
      const features = url.searchParams.get("features");
      const request: SlotRequest = {
        duration: integer(url, "duration") as number,
        attendees: integer(url, "attendees") as number,
        from: required(url, "from"),
        to: required(url, "to"),
        features: features ? features.split(",").filter((f) => f !== "") : [],
        step: integer(url, "step", true),
      };
      const slot = scheduler.findSlot(request);
      return slot === null ? json(404, { error: "no free slot in that window" }) : json(200, slot);
    } },
  ];

  return async (request: Request): Promise<Response> => {
    const url = new URL(request.url);
    const path = decodeURIComponent(url.pathname);
    let pathKnown = false;
    for (const route of routes) {
      const match = route.pattern.exec(path);
      if (!match) continue;
      pathKnown = true;
      if (route.method !== request.method) continue;
      try {
        return await route.run(match, url, request);
      } catch (err) {
        return errorResponse(err);
      }
    }
    if (pathKnown) return json(405, { error: `method ${request.method} not allowed on ${path}` });
    return json(404, { error: `no route for ${path}` });
  };
}
