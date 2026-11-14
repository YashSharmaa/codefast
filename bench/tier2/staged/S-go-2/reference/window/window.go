// Package window implements a fixed-window counter.
package window

import "time"

// Window counts the tokens taken in fixed windows of one length, aligned to
// the time it was created.
type Window struct {
	limit  int
	length time.Duration
	start  time.Time // start of the current window
	taken  int
}

// New returns a window limiter whose first window starts at now.
func New(limit int, length time.Duration, now time.Time) *Window {
	return &Window{limit: limit, length: length, start: now}
}

func (w *Window) advance(now time.Time) {
	end := w.start.Add(w.length)
	if now.Before(end) {
		return
	}
	k := now.Sub(w.start) / w.length
	w.start = w.start.Add(k * w.length)
	w.taken = 0
}

// Available returns the tokens left in the current window.
func (w *Window) Available(now time.Time) int {
	w.advance(now)
	return w.limit - w.taken
}

// Take takes n tokens from the current window if they are all available.
func (w *Window) Take(now time.Time, n int) bool {
	w.advance(now)
	if w.limit-w.taken < n {
		return false
	}
	w.taken += n
	return true
}

// RetryAfter returns 0 when n tokens are available, otherwise the whole
// seconds (at least 1) until the current window ends.
func (w *Window) RetryAfter(now time.Time, n int) int {
	w.advance(now)
	if w.limit-w.taken >= n {
		return 0
	}
	left := w.start.Add(w.length).Sub(now)
	secs := int((left + time.Second - 1) / time.Second)
	if secs < 1 {
		secs = 1
	}
	return secs
}
