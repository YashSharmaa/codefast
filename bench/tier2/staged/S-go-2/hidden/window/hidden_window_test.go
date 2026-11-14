package window_test

import (
	"testing"
	"time"

	"example.com/throttle/window"
)

var t0 = time.Date(2026, 3, 1, 12, 0, 0, 0, time.UTC)

func at(d time.Duration) time.Time { return t0.Add(d) }

func take(t *testing.T, w *window.Window, now time.Time, n int) {
	t.Helper()
	for i := 0; i < n; i++ {
		if !w.Take(now, 1) {
			t.Fatalf("Take %d failed", i+1)
		}
	}
}

func TestWindowStartsAtLimit(t *testing.T) {
	w := window.New(3, time.Minute, t0)
	if got := w.Available(t0); got != 3 {
		t.Fatalf("Available = %d, want 3", got)
	}
	take(t, w, t0, 2)
	if got := w.Available(at(time.Second)); got != 1 {
		t.Fatalf("Available = %d, want 1", got)
	}
	take(t, w, at(2*time.Second), 1)
	if w.Take(at(3 * time.Second), 1) {
		t.Fatalf("fourth Take in one window must fail")
	}
	if got := w.Available(at(59 * time.Second)); got != 0 {
		t.Fatalf("a failed Take must change nothing, Available = %d", got)
	}
}

func TestWindowStartsAgainAtTheBoundary(t *testing.T) {
	w := window.New(2, 10*time.Second, t0)
	take(t, w, t0, 2)
	if got := w.Available(at(10*time.Second - time.Nanosecond)); got != 0 {
		t.Fatalf("Available just before the end = %d, want 0", got)
	}
	if got := w.Available(at(10 * time.Second)); got != 2 {
		t.Fatalf("Available at the end = %d, want 2", got)
	}
	take(t, w, at(15*time.Second), 1)
	if got := w.Available(at(19 * time.Second)); got != 1 {
		t.Fatalf("Available = %d, want 1", got)
	}
}

func TestWindowAlignedToCreation(t *testing.T) {
	w := window.New(1, 10*time.Second, at(7*time.Second))
	take(t, w, at(8*time.Second), 1)
	if w.Take(at(17*time.Second - time.Nanosecond), 1) {
		t.Fatalf("the first window ends at 17s")
	}
	take(t, w, at(17*time.Second), 1)
	take(t, w, at(64*time.Second), 1)
	if got := w.RetryAfter(at(64 * time.Second), 1); got != 3 {
		t.Fatalf("RetryAfter = %d, want 3 (window [57s, 67s))", got)
	}
}

func TestWindowRetryAfter(t *testing.T) {
	w := window.New(2, 10*time.Second, t0)
	if got := w.RetryAfter(t0, 1); got != 0 {
		t.Fatalf("RetryAfter with tokens = %d", got)
	}
	take(t, w, t0, 2)
	checks := []struct {
		at   time.Duration
		want int
	}{
		{0, 10},
		{500 * time.Millisecond, 10},
		{time.Second, 9},
		{9 * time.Second, 1},
		{10*time.Second - time.Nanosecond, 1},
		{10 * time.Second, 0},
	}
	for _, c := range checks {
		if got := w.RetryAfter(at(c.at), 1); got != c.want {
			t.Fatalf("RetryAfter(+%v) = %d, want %d", c.at, got, c.want)
		}
	}
}

func TestWindowEarlierTimeStaysInCurrentWindow(t *testing.T) {
	w := window.New(1, 10*time.Second, t0)
	take(t, w, t0, 1)
	if got := w.Available(at(-30 * time.Second)); got != 0 {
		t.Fatalf("Available before the start = %d, want 0", got)
	}
	if got := w.RetryAfter(at(-5 * time.Second), 1); got != 15 {
		t.Fatalf("RetryAfter before the start = %d, want 15", got)
	}
	if got := w.Available(at(10 * time.Second)); got != 1 {
		t.Fatalf("Available = %d, want 1", got)
	}
}
