# roomplan: a meeting-room scheduler with an HTTP interface

`roomplan` keeps a set of meeting rooms and their bookings in memory, picks a room for a
request, finds free time, and exposes all of it through an HTTP handler built on the web
standard `Request` and `Response` classes. The project will be checked by tests written against
the interface below, so file paths, names, signatures, error classes, status codes and JSON
shapes must match exactly.

## Runtime and module rules

The code is TypeScript run directly by Node 26, which strips types. There is no build step and
no dependency. Relative imports carry the `.ts` extension (`import { Scheduler } from
"./scheduler.ts"`). Only erasable syntax is allowed: no `enum`, no `namespace`, no constructor
parameter properties, no `<T>value` type assertions (write `value as T`), and type-only
imports use `import type` or `import { type X }`. Files
are ES modules. A `package.json` is optional and, if present, has `"type": "module"` and no
dependencies.

## Files and exports

| File | Exports |
|---|---|
| `src/errors.ts` | `SchedulerError` (extends `Error`), and `ValidationError`, `NotFoundError`, `ConflictError`, `NoRoomError`, each extending `SchedulerError`. Each constructor takes the message first. `ConflictError` is built as `new ConflictError(message, conflicts = [])` and has `conflicts: string[]`. |
| `src/time.ts` | `parseTime(text: string): number`, `formatTime(minutes: number): string` |
| `src/intervals.ts` | `class IntervalIndex<T>` |
| `src/scheduler.ts` | `class Scheduler` and the types `Room`, `Booking`, `BookingRequest`, `SlotRequest`, `Slot` |
| `src/http.ts` | `createHandler(scheduler: Scheduler): (request: Request) => Promise<Response>` |

## Time

Times are written `YYYY-MM-DDTHH:MM` (UTC, no seconds, no zone). `parseTime` returns whole
minutes since `1970-01-01T00:00` and throws `ValidationError` for any other form or a date or
time that does not exist (`2026-02-30T10:00`, `2026-01-01T24:00`). `formatTime` is its inverse.

## IntervalIndex

Intervals are half-open: `[start, end)` and `[s, e)` overlap when `start < e` and `s < end`.

- `add(start: number, end: number, value: T): void` throws `RangeError` when `end <= start`.
- `remove(value: T): boolean` removes every interval holding that value (`===`) and says
  whether there was one.
- `overlapping(start: number, end: number): T[]` gives the values whose interval overlaps
  `[start, end)`, ordered by interval start, ties in insertion order.
- `gaps(from: number, to: number): Array<[number, number]>` gives the maximal parts of
  `[from, to)` that no interval covers, in ascending order.
- `size` (a getter) is the number of intervals held.

## Scheduler

```ts
type Room = { id: string; capacity: number; features: string[] };
type Booking = { id: string; room: string; start: string; end: string; attendees: number; title: string };
type BookingRequest = { room?: string; start: string; end: string; attendees: number; features?: string[]; title?: string };
type SlotRequest = { duration: number; attendees: number; from: string; to: string; features?: string[]; step?: number };
type Slot = { room: string; start: string; end: string };
```

`new Scheduler()` starts empty. Every method returns fresh plain objects, never internal state.

- `addRoom(room: { id: string; capacity: number; features?: string[] }): Room`. The id matches
  `^[A-Za-z0-9_-]+$` and the capacity is an integer of at least 1, else `ValidationError`. A
  room id already used gives `ConflictError` with `conflicts` empty. The returned and stored
  features are sorted and without duplicates.
- `rooms(): Room[]` lists rooms sorted by id with plain string comparison (`"B"` before `"a"`).
- `book(request: BookingRequest): Booking`. Checks in this order: times parse and `end` is after
  `start`, and `attendees` is an integer of at least 1 (`ValidationError`). With `room` given:
  unknown room is `NotFoundError`, too small a capacity or a missing requested feature is
  `ValidationError`, overlapping bookings give `ConflictError` whose `conflicts` holds their ids
  ordered by start time. Without `room`: the eligible rooms have enough capacity, every
  requested feature and no overlapping booking. The smallest capacity wins, ties go to the
  lower id by plain string comparison. No eligible room is `NoRoomError`. Booking ids are `b1`,
  `b2` and so on, counting successful bookings only and never reused. `title` defaults to `""`.
  Times in a returned booking are formatted with `formatTime`.
- `get(id: string): Booking` and `cancel(id: string): void` throw `NotFoundError` for an unknown
  or already cancelled id.
- `bookings(roomId: string, from: string, to: string): Booking[]` gives the room's bookings that
  overlap `[from, to)`, by start time. Unknown room is `NotFoundError`, `to <= from` is
  `ValidationError`, as is a time that does not parse.
- `freeSlots(roomId: string, from: string, to: string, minMinutes = 1): Array<{ start: string; end: string }>`
  gives the maximal free parts of `[from, to)` at least `minMinutes` long, ascending. Errors as
  for `bookings`.
- `findSlot(request: SlotRequest): Slot | null` tries the starts `from`, `from + step`,
  `from + 2*step` and so on (step defaults to 15 minutes) while `start + duration <= to`, and
  returns the first start at which some room is eligible as for `book` without `room`, with the
  room chosen by the same rule. `null` if none. `duration` and `step` must be integers of at
  least 1, `attendees` likewise, and the times must parse (`ValidationError`).
- `utilization(roomId: string, from: string, to: string): number` is the booked share of the
  minutes in `[from, to)`, from 0 to 1. Errors as for `bookings`.

## HTTP

`createHandler` returns an async function from `Request` to `Response`. Bodies are JSON with
`Content-Type: application/json`, except the empty body of a 204. Query values are read from the
URL, numbers in them must be digits only.

| Method and path | Success |
|---|---|
| `POST /rooms` with a room as body | 201 and the `Room` |
| `GET /rooms` | 200 and `rooms()` |
| `POST /bookings` with a `BookingRequest` as body | 201 and the `Booking` |
| `GET /bookings/{id}` | 200 and the `Booking` |
| `DELETE /bookings/{id}` | 204, empty body |
| `GET /rooms/{id}/bookings?from=&to=` | 200 and `bookings(...)` |
| `GET /rooms/{id}/free?from=&to=&min=` (`min` optional) | 200 and `freeSlots(...)` |
| `GET /slots?duration=&attendees=&from=&to=&features=&step=` | 200 and the `Slot`, or 404 if `findSlot` gives `null` |

`features` in `/slots` is a comma-separated list and optional, as is `step`. Errors: a body that
is not valid JSON or not an object, a missing or malformed query value, and `ValidationError`
give 400. `NotFoundError` gives 404, `ConflictError` 409, `NoRoomError` 422. Error bodies are
`{"error": "<message>"}`, and a 409 adds `"conflicts": [...]`. A path that matches no route is
404 and a known path with another method is 405, both with an error body.

## Constraints

Node 26, standard library only, no build step. The project will be checked by running
`node --test tests_hidden/` from the project root, so do not put files in `tests_hidden/`. Put
your own tests elsewhere, for example `test/`, as `*.test.ts` files that use `node:test` and
`node:assert/strict`.
