// Rooms, bookings and the rules for choosing a room and finding free time.

import { ConflictError, NoRoomError, NotFoundError, ValidationError } from "./errors.ts";
import { IntervalIndex } from "./intervals.ts";
import { formatTime, parseTime } from "./time.ts";

export type Room = { id: string; capacity: number; features: string[] };
export type Booking = { id: string; room: string; start: string; end: string; attendees: number; title: string };
export type BookingRequest = {
  room?: string;
  start: string;
  end: string;
  attendees: number;
  features?: string[];
  title?: string;
};
export type SlotRequest = {
  duration: number;
  attendees: number;
  from: string;
  to: string;
  features?: string[];
  step?: number;
};
export type Slot = { room: string; start: string; end: string };

type StoredBooking = { id: string; room: string; start: number; end: number; attendees: number; title: string };
type RoomState = { room: Room; index: IntervalIndex<StoredBooking> };

const ROOM_ID_RE = /^[A-Za-z0-9_-]+$/;

function positiveInteger(value: unknown, what: string): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 1) {
    throw new ValidationError(`${what} must be an integer of at least 1`);
  }
  return value;
}

function featureList(features: unknown): string[] {
  if (features === undefined) return [];
  if (!Array.isArray(features) || features.some((f) => typeof f !== "string")) {
    throw new ValidationError("features must be a list of strings");
  }
  return [...new Set(features as string[])].sort();
}

function window(from: string, to: string): [number, number] {
  const a = parseTime(from);
  const b = parseTime(to);
  if (b <= a) throw new ValidationError("the end of a time range must be after its start");
  return [a, b];
}

function publicBooking(b: StoredBooking): Booking {
  return { id: b.id, room: b.room, start: formatTime(b.start), end: formatTime(b.end), attendees: b.attendees, title: b.title };
}

function copyRoom(r: Room): Room {
  return { id: r.id, capacity: r.capacity, features: [...r.features] };
}

export class Scheduler {
  private roomStates = new Map<string, RoomState>();
  private all = new Map<string, StoredBooking>();
  private nextId = 1;

  addRoom(room: { id: string; capacity: number; features?: string[] }): Room {
    if (typeof room?.id !== "string" || !ROOM_ID_RE.test(room.id)) {
      throw new ValidationError("room id must match ^[A-Za-z0-9_-]+$");
    }
    positiveInteger(room.capacity, "capacity");
    if (this.roomStates.has(room.id)) {
      throw new ConflictError(`room ${room.id} already exists`, []);
    }
    const stored: Room = { id: room.id, capacity: room.capacity, features: featureList(room.features) };
    this.roomStates.set(room.id, { room: stored, index: new IntervalIndex<StoredBooking>() });
    return copyRoom(stored);
  }

  rooms(): Room[] {
    return [...this.roomStates.values()]
      .map((s) => copyRoom(s.room))
      .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  }

  private state(roomId: string): RoomState {
    const s = this.roomStates.get(roomId);
    if (!s) throw new NotFoundError(`no room ${roomId}`);
    return s;
  }

  private chooseRoom(start: number, end: number, attendees: number, features: string[]): RoomState | null {
    let best: RoomState | null = null;
    for (const s of this.roomStates.values()) {
      if (s.room.capacity < attendees) continue;
      if (!features.every((f) => s.room.features.includes(f))) continue;
      if (s.index.overlapping(start, end).length > 0) continue;
      if (
        best === null ||
        s.room.capacity < best.room.capacity ||
        (s.room.capacity === best.room.capacity && s.room.id < best.room.id)
      ) {
        best = s;
      }
    }
    return best;
  }

  book(request: BookingRequest): Booking {
    const [start, end] = window(request.start, request.end);
    const attendees = positiveInteger(request.attendees, "attendees");
    const features = featureList(request.features);
    let target: RoomState;
    if (request.room !== undefined) {
      target = this.state(request.room);
      if (target.room.capacity < attendees) {
        throw new ValidationError(`room ${request.room} holds only ${target.room.capacity}`);
      }
      const missing = features.filter((f) => !target.room.features.includes(f));
      if (missing.length > 0) {
        throw new ValidationError(`room ${request.room} lacks ${missing.join(", ")}`);
      }
      const clashes = target.index.overlapping(start, end);
      if (clashes.length > 0) {
        throw new ConflictError(`room ${request.room} is taken`, clashes.map((b) => b.id));
      }
    } else {
      const chosen = this.chooseRoom(start, end, attendees, features);
      if (chosen === null) throw new NoRoomError("no room is available for this request");
      target = chosen;
    }
    const booking: StoredBooking = {
      id: `b${this.nextId++}`,
      room: target.room.id,
      start,
      end,
      attendees,
      title: typeof request.title === "string" ? request.title : "",
    };
    target.index.add(start, end, booking);
    this.all.set(booking.id, booking);
    return publicBooking(booking);
  }

  get(id: string): Booking {
    const b = this.all.get(id);
    if (!b) throw new NotFoundError(`no booking ${id}`);
    return publicBooking(b);
  }

  cancel(id: string): void {
    const b = this.all.get(id);
    if (!b) throw new NotFoundError(`no booking ${id}`);
    this.all.delete(id);
    this.state(b.room).index.remove(b);
  }

  bookings(roomId: string, from: string, to: string): Booking[] {
    const s = this.state(roomId);
    const [a, b] = window(from, to);
    return s.index.overlapping(a, b).map(publicBooking);
  }

  freeSlots(roomId: string, from: string, to: string, minMinutes = 1): Array<{ start: string; end: string }> {
    const s = this.state(roomId);
    const [a, b] = window(from, to);
    return s.index
      .gaps(a, b)
      .filter(([gs, ge]) => ge - gs >= minMinutes)
      .map(([gs, ge]) => ({ start: formatTime(gs), end: formatTime(ge) }));
  }

  findSlot(request: SlotRequest): Slot | null {
    const duration = positiveInteger(request.duration, "duration");
    const attendees = positiveInteger(request.attendees, "attendees");
    const step = request.step === undefined ? 15 : positiveInteger(request.step, "step");
    const from = parseTime(request.from);
    const to = parseTime(request.to);
    const features = featureList(request.features);
    for (let start = from; start + duration <= to; start += step) {
      const chosen = this.chooseRoom(start, start + duration, attendees, features);
      if (chosen !== null) {
        return { room: chosen.room.id, start: formatTime(start), end: formatTime(start + duration) };
      }
    }
    return null;
  }

  utilization(roomId: string, from: string, to: string): number {
    const s = this.state(roomId);
    const [a, b] = window(from, to);
    let booked = 0;
    for (const bk of s.index.overlapping(a, b)) {
      booked += Math.min(bk.end, b) - Math.max(bk.start, a);
    }
    return booked / (b - a);
  }
}
