package bucket_test

import (
	"testing"
	"time"

	"example.com/throttle/bucket"
)

func TestBucketTakeSeveral(t *testing.T) {
	b := bucket.New(1, time.Minute, 5, t0)
	if !b.Take(t0, 3) || b.Available(t0) != 2 {
		t.Fatalf("Take(3) from 5")
	}
	if b.Take(t0, 3) {
		t.Fatalf("Take(3) with 2 left must fail")
	}
	if b.Available(t0) != 2 || !b.Take(t0, 2) || b.Available(t0) != 0 {
		t.Fatalf("a failed Take must change nothing")
	}
	if b.Take(t0.Add(59*time.Second), 1) || !b.Take(t0.Add(60*time.Second), 1) {
		t.Fatalf("refill before Take")
	}
}

func TestBucketRetryAfterSeveral(t *testing.T) {
	b := bucket.New(2, time.Minute, 5, t0)
	drain(t, b, t0, 5)
	if got := b.RetryAfter(t0, 3); got != 90 {
		t.Fatalf("RetryAfter(3) = %d, want 90", got)
	}
	at := t0.Add(30 * time.Second)
	if got := b.RetryAfter(at, 3); got != 60 {
		t.Fatalf("RetryAfter(3) at 30s = %d, want 60", got)
	}
	if got := b.RetryAfter(at, 1); got != 0 {
		t.Fatalf("RetryAfter(1) at 30s = %d, want 0", got)
	}
	if got := b.RetryAfter(t0.Add(89*time.Second+time.Nanosecond), 3); got != 1 {
		t.Fatalf("RetryAfter(3) just before = %d, want 1", got)
	}
	if got := b.RetryAfter(t0.Add(90*time.Second), 3); got != 0 {
		t.Fatalf("RetryAfter(3) at 90s = %d, want 0", got)
	}
}

func TestBucketTakeOneIsTheOldTake(t *testing.T) {
	b := bucket.New(1, 10*time.Second, 2, t0)
	if !b.Take(t0, 1) || b.Available(t0) != 1 {
		t.Fatalf("Take(1) takes one token")
	}
	b.Take(t0, 1)
	if b.RetryAfter(t0, 1) != 10 {
		t.Fatalf("RetryAfter(1) = %d, want 10", b.RetryAfter(t0, 1))
	}
	if got := b.RetryAfter(t0, 2); got != 20 {
		t.Fatalf("RetryAfter(2) = %d, want 20", got)
	}
}
