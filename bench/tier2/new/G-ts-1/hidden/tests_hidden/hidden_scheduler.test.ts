import { test } from "node:test";
import assert from "node:assert/strict";
import { ConflictError, NoRoomError, NotFoundError, ValidationError } from "../src/errors.ts";
import { Scheduler } from "../src/scheduler.ts";

const D = "2026-03-02";
const t = (hm: string) => `${D}T${hm}`;

function office(): Scheduler {
  const s = new Scheduler();
  s.addRoom({ id: "c", capacity: 8 });
  s.addRoom({ id: "b", capacity: 8, features: ["tv"] });
  s.addRoom({ id: "a", capacity: 4 });
  return s;
}

test("addRoom normalizes and rooms are sorted copies", () => {
  const s = new Scheduler();
  const r = s.addRoom({ id: "a", capacity: 6, features: ["tv", "board", "tv"] });
  assert.deepEqual(r, { id: "a", capacity: 6, features: ["board", "tv"] });
  s.addRoom({ id: "B", capacity: 2 });
  s.addRoom({ id: "a-2_x", capacity: 3 });
  assert.deepEqual(s.rooms().map((x) => x.id), ["B", "a", "a-2_x"]);
  r.features.push("hacked");
  s.rooms()[1].features.push("hacked");
  assert.deepEqual(s.rooms()[1].features, ["board", "tv"]);
  assert.deepEqual(s.rooms()[0].features, []);
});

test("addRoom validation and duplicates", () => {
  const s = new Scheduler();
  for (const room of [{ id: "", capacity: 1 }, { id: "a b", capacity: 1 }, { id: "x/y", capacity: 1 },
    { id: "ok", capacity: 0 }, { id: "ok", capacity: 1.5 }, { id: "ok", capacity: "3" as unknown as number }]) {
    assert.throws(() => s.addRoom(room), ValidationError, JSON.stringify(room));
  }
  s.addRoom({ id: "ok", capacity: 1 });
  assert.throws(() => s.addRoom({ id: "ok", capacity: 5 }), (e: unknown) => e instanceof ConflictError && e.conflicts.length === 0);
});

test("book a named room, ids count up, title defaults to empty", () => {
  const s = office();
  const b1 = s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 2, title: "standup" });
  assert.deepEqual(b1, { id: "b1", room: "a", start: t("09:00"), end: t("10:00"), attendees: 2, title: "standup" });
  const b2 = s.book({ room: "a", start: t("10:00"), end: t("10:30"), attendees: 4 });
  assert.equal(b2.id, "b2");
  assert.equal(b2.title, "");
  assert.deepEqual(s.get("b1"), b1);
});

test("book validates times and attendees", () => {
  const s = office();
  const bad = [
    { start: t("10:00"), end: t("10:00"), attendees: 1 },
    { start: t("10:00"), end: t("09:00"), attendees: 1 },
    { start: t("09:00"), end: t("10:00"), attendees: 0 },
    { start: t("09:00"), end: t("10:00"), attendees: 2.5 },
    { start: "tomorrow", end: t("10:00"), attendees: 1 },
    { room: "a", start: t("09:00"), end: "2026-03-02T25:00", attendees: 1 },
  ];
  for (const req of bad) assert.throws(() => s.book(req), ValidationError, JSON.stringify(req));
});

test("book a named room: unknown, too small, missing feature", () => {
  const s = office();
  assert.throws(() => s.book({ room: "zzz", start: t("09:00"), end: t("10:00"), attendees: 1 }), NotFoundError);
  assert.throws(() => s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 5 }), ValidationError);
  assert.throws(() => s.book({ room: "c", start: t("09:00"), end: t("10:00"), attendees: 2, features: ["tv"] }), ValidationError);
  assert.equal(s.book({ room: "b", start: t("09:00"), end: t("10:00"), attendees: 8, features: ["tv"] }).room, "b");
});

test("conflicts list overlapping bookings by start, back-to-back is fine", () => {
  const s = office();
  s.book({ room: "a", start: t("11:00"), end: t("12:00"), attendees: 1 });
  s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  s.book({ room: "a", start: t("10:00"), end: t("11:00"), attendees: 1 });
  assert.throws(
    () => s.book({ room: "a", start: t("09:30"), end: t("11:30"), attendees: 1 }),
    (e: unknown) => e instanceof ConflictError && JSON.stringify(e.conflicts) === JSON.stringify(["b2", "b3", "b1"]),
  );
  assert.equal(s.book({ room: "a", start: t("12:00"), end: t("13:00"), attendees: 1 }).id, "b4");
  assert.equal(s.book({ room: "a", start: t("08:00"), end: t("09:00"), attendees: 1 }).id, "b5");
});

test("automatic choice: smallest capacity, then lowest id, with features and free time", () => {
  const s = office();
  assert.equal(s.book({ start: t("09:00"), end: t("10:00"), attendees: 3 }).room, "a");
  assert.equal(s.book({ start: t("09:00"), end: t("10:00"), attendees: 3 }).room, "b");
  assert.equal(s.book({ start: t("09:00"), end: t("10:00"), attendees: 3 }).room, "c");
  assert.equal(s.book({ start: t("10:00"), end: t("11:00"), attendees: 5 }).room, "b");
  assert.equal(s.book({ start: t("10:30"), end: t("11:00"), attendees: 2 }).room, "a");
  assert.equal(s.book({ start: t("11:00"), end: t("11:30"), attendees: 2, features: ["tv"] }).room, "b");
});

test("no room available, and failures do not use up ids", () => {
  const s = office();
  assert.throws(() => s.book({ start: t("09:00"), end: t("10:00"), attendees: 9 }), NoRoomError);
  s.book({ room: "b", start: t("09:00"), end: t("10:00"), attendees: 1 });
  assert.throws(() => s.book({ start: t("09:30"), end: t("10:00"), attendees: 1, features: ["tv"] }), NoRoomError);
  assert.throws(() => s.book({ start: t("09:30"), end: t("10:00"), attendees: 1, features: ["projector"] }), NoRoomError);
  assert.equal(s.book({ start: t("09:30"), end: t("10:00"), attendees: 1 }).id, "b2");
});

test("cancel frees time and ids are never reused", () => {
  const s = office();
  s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  s.cancel("b1");
  assert.throws(() => s.get("b1"), NotFoundError);
  assert.throws(() => s.cancel("b1"), NotFoundError);
  assert.throws(() => s.cancel("nope"), NotFoundError);
  const again = s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  assert.equal(again.id, "b2");
  assert.deepEqual(s.bookings("a", t("00:00"), t("23:59")).map((b) => b.id), ["b2"]);
});

test("bookings in a window", () => {
  const s = office();
  s.book({ room: "c", start: t("13:00"), end: t("14:00"), attendees: 1 });
  s.book({ room: "c", start: t("09:00"), end: t("10:00"), attendees: 1 });
  s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  assert.deepEqual(s.bookings("c", t("08:00"), t("18:00")).map((b) => b.id), ["b2", "b1"]);
  assert.deepEqual(s.bookings("c", t("10:00"), t("13:00")), []);
  assert.deepEqual(s.bookings("c", t("09:59"), t("13:01")).map((b) => b.id), ["b2", "b1"]);
  assert.throws(() => s.bookings("zzz", t("08:00"), t("09:00")), NotFoundError);
  assert.throws(() => s.bookings("c", t("09:00"), t("09:00")), ValidationError);
  assert.throws(() => s.bookings("c", "9am", t("09:00")), ValidationError);
});

test("freeSlots with and without a minimum length", () => {
  const s = office();
  s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  s.book({ room: "a", start: t("10:20"), end: t("11:00"), attendees: 1 });
  s.book({ room: "a", start: t("11:00"), end: t("11:30"), attendees: 1 });
  assert.deepEqual(s.freeSlots("a", t("08:00"), t("12:00")), [
    { start: t("08:00"), end: t("09:00") },
    { start: t("10:00"), end: t("10:20") },
    { start: t("11:30"), end: t("12:00") },
  ]);
  assert.deepEqual(s.freeSlots("a", t("08:30"), t("12:00"), 30), [
    { start: t("08:30"), end: t("09:00") },
    { start: t("11:30"), end: t("12:00") },
  ]);
  assert.deepEqual(s.freeSlots("a", t("09:15"), t("09:45")), []);
  assert.throws(() => s.freeSlots("zzz", t("08:00"), t("09:00")), NotFoundError);
});

function busyOffice(): Scheduler {
  const s = office();
  s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  s.book({ room: "b", start: t("09:00"), end: t("09:30"), attendees: 1 });
  s.book({ room: "c", start: t("09:00"), end: t("10:30"), attendees: 1 });
  return s;
}

test("findSlot walks the window in steps and picks the room by the booking rule", () => {
  const s = busyOffice();
  assert.deepEqual(s.findSlot({ duration: 30, attendees: 2, from: t("09:00"), to: t("12:00") }),
    { room: "b", start: t("09:30"), end: t("10:00") });
  assert.deepEqual(s.findSlot({ duration: 60, attendees: 2, from: t("09:00"), to: t("10:30") }),
    { room: "b", start: t("09:30"), end: t("10:30") });
  assert.deepEqual(s.findSlot({ duration: 30, attendees: 2, from: t("09:05"), to: t("12:00"), step: 20 }),
    { room: "b", start: t("09:45"), end: t("10:15") });
  assert.deepEqual(s.findSlot({ duration: 30, attendees: 2, from: t("10:00"), to: t("12:00") }),
    { room: "a", start: t("10:00"), end: t("10:30") });
  assert.deepEqual(s.findSlot({ duration: 15, attendees: 5, from: t("09:00"), to: t("12:00"), features: ["tv"] }),
    { room: "b", start: t("09:30"), end: t("09:45") });
});

test("findSlot returns null and validates", () => {
  const s = busyOffice();
  assert.equal(s.findSlot({ duration: 30, attendees: 20, from: t("09:00"), to: t("12:00") }), null);
  assert.equal(s.findSlot({ duration: 120, attendees: 1, from: t("09:00"), to: t("10:00") }), null);
  assert.equal(s.findSlot({ duration: 30, attendees: 1, from: t("09:00"), to: t("09:29"), step: 1 }), null);
  for (const bad of [{ duration: 0, attendees: 1 }, { duration: 30, attendees: 0 }, { duration: 30, attendees: 1, step: 0 },
    { duration: 7.5, attendees: 1 }]) {
    assert.throws(() => s.findSlot({ from: t("09:00"), to: t("12:00"), ...bad }), ValidationError, JSON.stringify(bad));
  }
  assert.throws(() => s.findSlot({ duration: 30, attendees: 1, from: "nine", to: t("12:00") }), ValidationError);
});

test("utilization clips bookings to the window", () => {
  const s = busyOffice();
  assert.equal(s.utilization("a", t("08:30"), t("10:30")), 0.5);
  assert.equal(s.utilization("a", t("09:30"), t("09:45")), 1);
  assert.equal(s.utilization("a", t("11:00"), t("12:00")), 0);
  assert.equal(s.utilization("c", t("10:00"), t("11:00")), 0.5);
  assert.throws(() => s.utilization("zzz", t("08:00"), t("09:00")), NotFoundError);
  assert.throws(() => s.utilization("a", t("10:00"), t("09:00")), ValidationError);
});

test("returned bookings are fresh objects", () => {
  const s = office();
  const b = s.book({ room: "a", start: t("09:00"), end: t("10:00"), attendees: 1 });
  b.title = "changed";
  b.start = t("00:00");
  assert.equal(s.get("b1").title, "");
  assert.equal(s.bookings("a", t("08:00"), t("11:00"))[0].start, t("09:00"));
});
