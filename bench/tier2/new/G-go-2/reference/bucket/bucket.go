// Package bucket implements an exact integer token bucket.
package bucket

import "time"

// Bucket is a token bucket. One token is worth `unit` units, where unit is
// the window length in nanoseconds, and the level grows by `limit` units per
// nanosecond, so limit tokens are added per window.
type Bucket struct {
	limit int64
	unit  int64
	cap   int64
	level int64
	last  time.Time
}

// New returns a full bucket created at now.
func New(limit int, window time.Duration, burst int, now time.Time) *Bucket {
	unit := window.Nanoseconds()
	return &Bucket{
		limit: int64(limit),
		unit:  unit,
		cap:   int64(burst) * unit,
		level: int64(burst) * unit,
		last:  now,
	}
}

func (b *Bucket) refill(now time.Time) {
	if now.Before(b.last) {
		return
	}
	elapsed := now.Sub(b.last).Nanoseconds()
	b.last = now
	need := b.cap - b.level
	if need <= 0 {
		return
	}
	if elapsed > need/b.limit {
		b.level = b.cap
		return
	}
	b.level += elapsed * b.limit
	if b.level > b.cap {
		b.level = b.cap
	}
}

// Available refills the bucket and returns the number of whole tokens.
func (b *Bucket) Available(now time.Time) int {
	b.refill(now)
	return int(b.level / b.unit)
}

// Take refills the bucket and removes one token if a whole token is available.
func (b *Bucket) Take(now time.Time) bool {
	b.refill(now)
	if b.level < b.unit {
		return false
	}
	b.level -= b.unit
	return true
}

// RetryAfter refills the bucket and returns 0 when a token is available,
// otherwise the whole seconds (at least 1) until one will be.
func (b *Bucket) RetryAfter(now time.Time) int {
	b.refill(now)
	if b.level >= b.unit {
		return 0
	}
	ns := (b.unit - b.level + b.limit - 1) / b.limit
	secs := (ns + int64(time.Second) - 1) / int64(time.Second)
	if secs < 1 {
		secs = 1
	}
	return int(secs)
}
