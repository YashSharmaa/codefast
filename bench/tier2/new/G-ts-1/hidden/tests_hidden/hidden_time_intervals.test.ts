import { test } from "node:test";
import assert from "node:assert/strict";
import { ConflictError, NoRoomError, NotFoundError, SchedulerError, ValidationError } from "../src/errors.ts";
import { formatTime, parseTime } from "../src/time.ts";
import { IntervalIndex } from "../src/intervals.ts";

test("parseTime and formatTime round trip", () => {
  assert.equal(parseTime("1970-01-01T00:00"), 0);
  assert.equal(parseTime("1970-01-02T01:30"), 1530);
  assert.equal(formatTime(1530), "1970-01-02T01:30");
  assert.equal(formatTime(parseTime("2024-02-29T23:59")), "2024-02-29T23:59");
  assert.equal(parseTime("2026-03-02T10:00") - parseTime("2026-03-01T10:00"), 1440);
});

test("parseTime rejects malformed or impossible times", () => {
  for (const text of ["2026-02-30T10:00", "2026-01-01T24:00", "2026-01-01T10:60", "2026-1-01T10:00",
    "2026-01-01 10:00", "2026-01-01T10:00:00", "2026-01-01T10:00Z", "", "2023-02-29T00:00", "2026-13-01T00:00"]) {
    assert.throws(() => parseTime(text), ValidationError, text);
  }
});

test("error classes", () => {
  for (const E of [ValidationError, NotFoundError, NoRoomError]) {
    const e = new E("boom");
    assert.ok(e instanceof SchedulerError);
    assert.ok(e instanceof Error);
    assert.equal(e.message, "boom");
  }
  const c = new ConflictError("taken", ["b1"]);
  assert.ok(c instanceof SchedulerError);
  assert.deepEqual(c.conflicts, ["b1"]);
});

test("IntervalIndex overlap is half-open and ordered by start then insertion", () => {
  const idx = new IntervalIndex<string>();
  idx.add(10, 20, "a");
  idx.add(20, 30, "b");
  idx.add(5, 15, "c");
  idx.add(10, 12, "d");
  assert.deepEqual(idx.overlapping(15, 20), ["a"]);
  assert.deepEqual(idx.overlapping(0, 100), ["c", "a", "d", "b"]);
  assert.deepEqual(idx.overlapping(30, 40), []);
  assert.deepEqual(idx.overlapping(29, 30), ["b"]);
  assert.equal(idx.size, 4);
});

test("IntervalIndex add rejects empty intervals and remove drops every match", () => {
  const idx = new IntervalIndex<object>();
  const x = {};
  const y = {};
  assert.throws(() => idx.add(5, 5, x), RangeError);
  assert.throws(() => idx.add(6, 5, x), RangeError);
  idx.add(1, 2, x);
  idx.add(3, 4, x);
  idx.add(2, 3, y);
  assert.equal(idx.size, 3);
  assert.equal(idx.remove({}), false);
  assert.equal(idx.remove(x), true);
  assert.equal(idx.size, 1);
  assert.deepEqual(idx.overlapping(0, 10), [y]);
});

test("IntervalIndex gaps", () => {
  const idx = new IntervalIndex<number>();
  assert.deepEqual(idx.gaps(0, 10), [[0, 10]]);
  idx.add(2, 4, 1);
  idx.add(3, 6, 2);
  idx.add(8, 12, 3);
  assert.deepEqual(idx.gaps(0, 10), [[0, 2], [6, 8]]);
  assert.deepEqual(idx.gaps(3, 7), [[6, 7]]);
  assert.deepEqual(idx.gaps(8, 12), []);
  assert.deepEqual(idx.gaps(-5, 1), [[-5, 1]]);
});
