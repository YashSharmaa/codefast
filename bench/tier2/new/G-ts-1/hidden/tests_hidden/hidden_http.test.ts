import { test } from "node:test";
import assert from "node:assert/strict";
import { Scheduler } from "../src/scheduler.ts";
import { createHandler } from "../src/http.ts";

const D = "2026-03-02";
const t = (hm: string) => `${D}T${hm}`;

type Call = { status: number; body: any; type: string | null; text: string };

async function setup() {
  const handler = createHandler(new Scheduler());
  const call = async (method: string, path: string, body?: unknown, raw?: string): Promise<Call> => {
    const init: RequestInit = { method };
    if (raw !== undefined) init.body = raw;
    else if (body !== undefined) init.body = JSON.stringify(body);
    const res = await handler(new Request(`http://localhost${path}`, init));
    const text = await res.text();
    return { status: res.status, body: text ? JSON.parse(text) : null, type: res.headers.get("content-type"), text };
  };
  await call("POST", "/rooms", { id: "a", capacity: 4 });
  await call("POST", "/rooms", { id: "b", capacity: 8, features: ["tv"] });
  return call;
}

test("rooms: create and list", async () => {
  const call = await setup();
  const r = await call("POST", "/rooms", { id: "c", capacity: 2, features: ["tv", "board"] });
  assert.equal(r.status, 201);
  assert.match(r.type ?? "", /^application\/json/);
  assert.deepEqual(r.body, { id: "c", capacity: 2, features: ["board", "tv"] });
  const list = await call("GET", "/rooms");
  assert.equal(list.status, 200);
  assert.deepEqual(list.body.map((x: any) => x.id), ["a", "b", "c"]);
});

test("bookings: create, read, delete", async () => {
  const call = await setup();
  const created = await call("POST", "/bookings", { start: t("09:00"), end: t("10:00"), attendees: 3, title: "plan" });
  assert.equal(created.status, 201);
  assert.deepEqual(created.body, { id: "b1", room: "a", start: t("09:00"), end: t("10:00"), attendees: 3, title: "plan" });
  assert.deepEqual((await call("GET", "/bookings/b1")).body, created.body);
  const del = await call("DELETE", "/bookings/b1");
  assert.equal(del.status, 204);
  assert.equal(del.text, "");
  const gone = await call("GET", "/bookings/b1");
  assert.equal(gone.status, 404);
  assert.equal(typeof gone.body.error, "string");
  assert.equal((await call("DELETE", "/bookings/b1")).status, 404);
});

test("409 conflicts and 422 no room", async () => {
  const call = await setup();
  await call("POST", "/bookings", { room: "b", start: t("09:00"), end: t("10:00"), attendees: 1 });
  await call("POST", "/bookings", { room: "b", start: t("10:00"), end: t("11:00"), attendees: 1 });
  const clash = await call("POST", "/bookings", { room: "b", start: t("09:30"), end: t("10:30"), attendees: 1 });
  assert.equal(clash.status, 409);
  assert.deepEqual(clash.body.conflicts, ["b1", "b2"]);
  assert.equal(typeof clash.body.error, "string");
  const dup = await call("POST", "/rooms", { id: "a", capacity: 1 });
  assert.equal(dup.status, 409);
  assert.deepEqual(dup.body.conflicts, []);
  const none = await call("POST", "/bookings", { start: t("09:00"), end: t("10:00"), attendees: 2, features: ["tv"] });
  assert.equal(none.status, 422);
  assert.equal(typeof none.body.error, "string");
});

test("400 for bad bodies, bad values and bad queries", async () => {
  const call = await setup();
  const cases: Array<[string, string, unknown, string | undefined]> = [
    ["POST", "/rooms", undefined, "{not json"],
    ["POST", "/bookings", undefined, "[1, 2]"],
    ["POST", "/bookings", undefined, "null"],
    ["POST", "/rooms", { id: "bad id", capacity: 1 }, undefined],
    ["POST", "/bookings", { start: "soon", end: t("10:00"), attendees: 1 }, undefined],
    ["POST", "/bookings", { room: "a", start: t("10:00"), end: t("11:00"), attendees: 9 }, undefined],
    ["GET", `/rooms/a/bookings?from=${t("09:00")}`, undefined, undefined],
    ["GET", `/rooms/a/free?from=${t("09:00")}&to=${t("10:00")}&min=ten`, undefined, undefined],
    ["GET", `/slots?duration=30&from=${t("09:00")}&to=${t("10:00")}`, undefined, undefined],
    ["GET", `/slots?duration=-5&attendees=1&from=${t("09:00")}&to=${t("10:00")}`, undefined, undefined],
    ["GET", `/rooms/a/bookings?from=${t("10:00")}&to=${t("09:00")}`, undefined, undefined],
  ];
  for (const [method, path, body, raw] of cases) {
    const r = await call(method, path, body, raw);
    assert.equal(r.status, 400, `${method} ${path} ${raw ?? JSON.stringify(body)}`);
    assert.equal(typeof r.body.error, "string");
  }
});

test("room bookings and free time over HTTP", async () => {
  const call = await setup();
  await call("POST", "/bookings", { room: "a", start: t("13:00"), end: t("14:00"), attendees: 1 });
  await call("POST", "/bookings", { room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  const list = await call("GET", `/rooms/a/bookings?from=${t("08:00")}&to=${t("18:00")}`);
  assert.equal(list.status, 200);
  assert.deepEqual(list.body.map((b: any) => b.id), ["b2", "b1"]);
  const free = await call("GET", `/rooms/a/free?from=${t("08:30")}&to=${t("14:30")}&min=45`);
  assert.deepEqual(free.body, [{ start: t("10:00"), end: t("13:00") }]);
  const all = await call("GET", `/rooms/a/free?from=${t("08:30")}&to=${t("14:30")}`);
  assert.equal(all.body.length, 3);
  const missing = await call("GET", `/rooms/zzz/free?from=${t("08:30")}&to=${t("14:30")}`);
  assert.equal(missing.status, 404);
});

test("slots over HTTP", async () => {
  const call = await setup();
  await call("POST", "/bookings", { room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  await call("POST", "/bookings", { room: "b", start: t("09:00"), end: t("09:30"), attendees: 1 });
  const r = await call("GET", `/slots?duration=30&attendees=2&from=${t("09:00")}&to=${t("12:00")}`);
  assert.equal(r.status, 200);
  assert.deepEqual(r.body, { room: "b", start: t("09:30"), end: t("10:00") });
  const tv = await call("GET", `/slots?duration=60&attendees=1&from=${t("09:10")}&to=${t("12:00")}&features=tv&step=10`);
  assert.deepEqual(tv.body, { room: "b", start: t("09:30"), end: t("10:30") });
  const none = await call("GET", `/slots?duration=30&attendees=50&from=${t("09:00")}&to=${t("12:00")}`);
  assert.equal(none.status, 404);
  assert.equal(typeof none.body.error, "string");
});

test("unknown paths are 404 and wrong methods 405", async () => {
  const call = await setup();
  for (const [method, path] of [["GET", "/nothing"], ["GET", "/rooms/a/b/c"], ["POST", "/"]]) {
    const r = await call(method, path);
    assert.equal(r.status, 404, `${method} ${path}`);
    assert.equal(typeof r.body.error, "string");
  }
  for (const [method, path] of [["PUT", "/rooms"], ["POST", "/slots"], ["DELETE", "/rooms"], ["PATCH", "/bookings/b1"]]) {
    const r = await call(method, path);
    assert.equal(r.status, 405, `${method} ${path}`);
    assert.equal(typeof r.body.error, "string");
  }
});
