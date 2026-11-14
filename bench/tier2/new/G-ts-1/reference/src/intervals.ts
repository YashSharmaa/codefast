// A sorted index of half-open intervals [start, end) carrying values.

type Entry<T> = { start: number; end: number; value: T; seq: number };

export class IntervalIndex<T> {
  private entries: Entry<T>[] = [];
  private seq = 0;

  get size(): number {
    return this.entries.length;
  }

  add(start: number, end: number, value: T): void {
    if (!(end > start)) {
      throw new RangeError(`interval end ${end} must be after start ${start}`);
    }
    const entry = { start, end, value, seq: this.seq++ };
    // keep entries ordered by start, then insertion order
    let lo = 0;
    let hi = this.entries.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.entries[mid].start <= start) lo = mid + 1;
      else hi = mid;
    }
    this.entries.splice(lo, 0, entry);
  }

  remove(value: T): boolean {
    const before = this.entries.length;
    this.entries = this.entries.filter((e) => e.value !== value);
    return this.entries.length !== before;
  }

  overlapping(start: number, end: number): T[] {
    const out: T[] = [];
    for (const e of this.entries) {
      if (e.start >= end) break;
      if (e.end > start) out.push(e.value);
    }
    return out;
  }

  gaps(from: number, to: number): Array<[number, number]> {
    const out: Array<[number, number]> = [];
    let cursor = from;
    for (const e of this.entries) {
      if (e.start >= to) break;
      if (e.end <= cursor) continue;
      if (e.start > cursor) out.push([cursor, Math.min(e.start, to)]);
      cursor = Math.max(cursor, e.end);
      if (cursor >= to) break;
    }
    if (cursor < to) out.push([cursor, to]);
    return out;
  }
}
