// Package engine applies rate-limit policies to requests.
package engine

import (
	"sync"

	"example.com/throttle/bucket"
	"example.com/throttle/clock"
	"example.com/throttle/policy"
)

// Request describes one incoming request.
type Request struct {
	Method string
	Path   string
	User   string
	IP     string
}

// Decision is the outcome of Check.
type Decision struct {
	Allowed    bool
	Matched    []string
	Denied     string
	RetryAfter int
	Remaining  int
}

// Engine holds the policies and one bucket per policy and key value.
type Engine struct {
	mu       sync.Mutex
	policies []policy.Policy
	clock    clock.Clock
	buckets  map[[2]string]*bucket.Bucket
}

// New returns an engine for the policies, reading time from c.
func New(policies []policy.Policy, c clock.Clock) *Engine {
	return &Engine{
		policies: append([]policy.Policy(nil), policies...),
		clock:    c,
		buckets:  map[[2]string]*bucket.Bucket{},
	}
}

// Policies returns the engine's policies in file order.
func (e *Engine) Policies() []policy.Policy {
	return append([]policy.Policy(nil), e.policies...)
}

// KeyFor returns the bucket key value of r under p.
func KeyFor(p policy.Policy, r Request) string {
	switch p.By {
	case "user":
		if r.User == "" {
			return "anonymous"
		}
		return r.User
	case "global":
		return ""
	}
	return r.IP
}

// Check decides whether r is allowed and takes tokens when it is.
func (e *Engine) Check(r Request) Decision {
	e.mu.Lock()
	defer e.mu.Unlock()
	now := e.clock.Now()
	d := Decision{Allowed: true, Matched: []string{}, Remaining: -1}
	var hit []*bucket.Bucket
	for _, p := range e.policies {
		if !p.Matches(r.Method, r.Path) {
			continue
		}
		k := [2]string{p.Name, KeyFor(p, r)}
		b := e.buckets[k]
		if b == nil {
			b = bucket.New(p.Limit, p.Window, p.Burst, now)
			e.buckets[k] = b
		}
		d.Matched = append(d.Matched, p.Name)
		hit = append(hit, b)
		if b.Available(now) < 1 {
			if d.Allowed {
				d.Allowed = false
				d.Denied = p.Name
			}
			d.RetryAfter = max(d.RetryAfter, b.RetryAfter(now))
		}
	}
	for _, b := range hit {
		if d.Allowed {
			b.Take(now)
		}
		left := b.Available(now)
		if d.Remaining < 0 || left < d.Remaining {
			d.Remaining = left
		}
	}
	return d
}
