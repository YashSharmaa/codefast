package window_test

import (
	"testing"
	"time"

	"example.com/throttle/window"
)

func TestWindowTakeAndRetryAfterSeveral(t *testing.T) {
	w := window.New(5, 10*time.Second, t0)
	if !w.Take(t0, 3) || w.Available(t0) != 2 {
		t.Fatalf("Take(3) from 5")
	}
	if w.Take(at(time.Second), 3) || w.Available(at(time.Second)) != 2 {
		t.Fatalf("Take(3) with 2 left must fail and change nothing")
	}
	if got := w.RetryAfter(at(time.Second), 2); got != 0 {
		t.Fatalf("RetryAfter(2) with 2 left = %d", got)
	}
	if got := w.RetryAfter(at(2500*time.Millisecond), 3); got != 8 {
		t.Fatalf("RetryAfter(3) = %d, want 8", got)
	}
	if !w.Take(at(10*time.Second), 5) || w.Available(at(10*time.Second)) != 0 {
		t.Fatalf("a new window holds the whole limit")
	}
}
