package bucket_test

import (
	"testing"
	"time"

	"example.com/throttle/bucket"
)

var t0 = time.Date(2026, 3, 1, 12, 0, 0, 0, time.UTC)

func drain(t *testing.T, b *bucket.Bucket, now time.Time, n int) {
	t.Helper()
	for i := 0; i < n; i++ {
		if !b.Take(now) {
			t.Fatalf("Take %d failed", i+1)
		}
	}
}

func TestBucketStartsFullAtBurst(t *testing.T) {
	b := bucket.New(10, time.Minute, 4, t0)
	if got := b.Available(t0); got != 4 {
		t.Fatalf("Available = %d, want 4", got)
	}
	drain(t, b, t0, 4)
	if b.Take(t0) {
		t.Fatalf("fifth Take must fail")
	}
	if got := b.Available(t0); got != 0 {
		t.Fatalf("Available = %d, want 0", got)
	}
}

func TestBucketRefillIsExactInNanoseconds(t *testing.T) {
	b := bucket.New(3, time.Second, 3, t0)
	drain(t, b, t0, 3)
	if got := b.Available(t0.Add(333333333 * time.Nanosecond)); got != 0 {
		t.Fatalf("after 333333333ns Available = %d, want 0", got)
	}
	if got := b.Available(t0.Add(333333334 * time.Nanosecond)); got != 1 {
		t.Fatalf("after 333333334ns Available = %d, want 1", got)
	}
	if got := b.Available(t0.Add(666666667 * time.Nanosecond)); got != 2 {
		t.Fatalf("after 666666667ns Available = %d, want 2", got)
	}
}

func TestBucketCapsAtBurst(t *testing.T) {
	b := bucket.New(5, time.Second, 2, t0)
	drain(t, b, t0, 2)
	if got := b.Available(t0.Add(10 * time.Second)); got != 2 {
		t.Fatalf("Available = %d, want burst 2", got)
	}
	drain(t, b, t0.Add(10*time.Second), 2)
	if b.Take(t0.Add(10 * time.Second)) {
		t.Fatalf("level must not exceed the burst")
	}
}

func TestBucketPartialTokensAccumulate(t *testing.T) {
	b := bucket.New(2, time.Second, 1, t0)
	drain(t, b, t0, 1)
	now := t0.Add(400 * time.Millisecond)
	if b.Take(now) {
		t.Fatalf("0.8 token must not be enough")
	}
	if got := b.Available(t0.Add(499999999 * time.Nanosecond)); got != 0 {
		t.Fatalf("Available = %d, want 0", got)
	}
	if !b.Take(t0.Add(500 * time.Millisecond)) {
		t.Fatalf("a whole token must be there after 0.5s")
	}
	if got := b.Available(t0.Add(999 * time.Millisecond)); got != 0 {
		t.Fatalf("Available = %d after take, want 0", got)
	}
	if got := b.Available(t0.Add(time.Second)); got != 1 {
		t.Fatalf("Available = %d, want 1", got)
	}
}

func TestBucketRetryAfterRoundsUpToSeconds(t *testing.T) {
	b := bucket.New(1, time.Minute, 1, t0)
	if got := b.RetryAfter(t0); got != 0 {
		t.Fatalf("RetryAfter with a token = %d, want 0", got)
	}
	drain(t, b, t0, 1)
	checks := []struct {
		at   time.Duration
		want int
	}{
		{0, 60},
		{30*time.Second - time.Nanosecond, 31},
		{30 * time.Second, 30},
		{30*time.Second + time.Nanosecond, 30},
		{59*time.Second + 500*time.Millisecond, 1},
		{60*time.Second - time.Nanosecond, 1},
		{60 * time.Second, 0},
	}
	for _, c := range checks {
		if got := b.RetryAfter(t0.Add(c.at)); got != c.want {
			t.Fatalf("RetryAfter at +%v = %d, want %d", c.at, got, c.want)
		}
	}
}

func TestBucketRetryAfterMinimumOneSecond(t *testing.T) {
	b := bucket.New(10, time.Second, 1, t0)
	drain(t, b, t0, 1)
	if got := b.RetryAfter(t0); got != 1 {
		t.Fatalf("RetryAfter = %d, want 1 (0.1s rounds up)", got)
	}
	b2 := bucket.New(7, time.Hour, 7, t0)
	drain(t, b2, t0, 7)
	if got := b2.RetryAfter(t0); got != 515 {
		t.Fatalf("RetryAfter = %d, want ceil(3600/7) = 515", got)
	}
}

func TestBucketTimeGoingBackwardsChangesNothing(t *testing.T) {
	b := bucket.New(1, time.Second, 1, t0)
	drain(t, b, t0, 1)
	if got := b.Available(t0.Add(-10 * time.Second)); got != 0 {
		t.Fatalf("earlier time must not refill, got %d", got)
	}
	if got := b.Available(t0.Add(999 * time.Millisecond)); got != 0 {
		t.Fatalf("Available = %d, want 0", got)
	}
	if got := b.Available(t0.Add(time.Second)); got != 1 {
		t.Fatalf("refill must count from the last refill time, got %d", got)
	}
}

func TestBucketLargeGapDoesNotOverflow(t *testing.T) {
	b := bucket.New(100000, 24*time.Hour, 100000, t0)
	drain(t, b, t0, 5)
	if got := b.Available(t0); got != 99995 {
		t.Fatalf("Available = %d, want 99995", got)
	}
	later := t0.Add(1000 * time.Hour)
	if got := b.Available(later); got != 100000 {
		t.Fatalf("Available after 1000h = %d, want 100000", got)
	}
	if !b.Take(later) || b.Available(later) != 99999 {
		t.Fatalf("bucket broken after a long gap")
	}
}

func TestBucketFailedTakeChangesNothing(t *testing.T) {
	b := bucket.New(1, 10*time.Second, 1, t0)
	drain(t, b, t0, 1)
	for i := 0; i < 5; i++ {
		if b.Take(t0.Add(time.Duration(i) * time.Second)) {
			t.Fatalf("Take must fail before 10s")
		}
	}
	if !b.Take(t0.Add(10 * time.Second)) {
		t.Fatalf("failed takes must not consume anything")
	}
}
