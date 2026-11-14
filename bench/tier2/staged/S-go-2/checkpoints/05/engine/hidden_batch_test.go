package engine_test

import (
	"reflect"
	"sync"
	"testing"
	"time"

	"example.com/throttle/clock"
	"example.com/throttle/engine"
	"example.com/throttle/policy"
)

// ticking is a clock that moves one second forward each time it is read.
type ticking struct {
	mu    sync.Mutex
	now   time.Time
	reads int
}

func (c *ticking) Now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.reads++
	c.now = c.now.Add(time.Second)
	return c.now
}

func tickingEngine(t *testing.T, src string) (*engine.Engine, *ticking) {
	t.Helper()
	ps, err := policy.Parse(src)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	c := &ticking{now: t0}
	var _ clock.Clock = c
	return engine.New(ps, c), c
}

func TestCheckAllInOrder(t *testing.T) {
	e, _ := newEngine(t, "policy one * /x 1/1m\npolicy all * /** 5/1m\n")
	got := e.CheckAll([]engine.Request{req("GET", "/x", "", "a"), req("GET", "/y", "", "a"), req("GET", "/x", "", "a")})
	want := []engine.Decision{
		{Allowed: true, Matched: []string{"one", "all"}, Remaining: 0},
		{Allowed: true, Matched: []string{"all"}, Remaining: 3},
		{Allowed: false, Matched: []string{"one", "all"}, Denied: "one", RetryAfter: 60, Remaining: 0},
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("CheckAll = %+v\nwant %+v", got, want)
	}
	if d := e.Check(req("GET", "/y", "", "a")); !d.Allowed || d.Remaining != 2 {
		t.Fatalf("CheckAll must use the same buckets as Check: %+v", d)
	}
}

func TestCheckAllReadsTheClockOnce(t *testing.T) {
	e, c := tickingEngine(t, "policy p * /** 1/1s\n")
	before := c.reads
	got := e.CheckAll([]engine.Request{req("GET", "/", "", "k"), req("GET", "/", "", "k"), req("GET", "/", "", "k")})
	if c.reads-before != 1 {
		t.Fatalf("CheckAll read the clock %d times, want 1", c.reads-before)
	}
	if !got[0].Allowed || got[1].Allowed || got[2].Allowed || got[1].RetryAfter != 1 {
		t.Fatalf("CheckAll = %+v", got)
	}
	e2, c2 := tickingEngine(t, "policy p * /** 1/1s\n")
	before = c2.reads
	for i := 0; i < 3; i++ {
		if d := e2.Check(req("GET", "/", "", "k")); !d.Allowed {
			t.Fatalf("Check %d at a new second: %+v", i+1, d)
		}
	}
	if c2.reads-before != 3 {
		t.Fatalf("Check read the clock %d times, want 3", c2.reads-before)
	}
}

func TestCheckAllEmpty(t *testing.T) {
	e, _ := newEngine(t, "policy p * /** 1/1s\n")
	for _, rs := range [][]engine.Request{nil, {}} {
		if got := e.CheckAll(rs); got == nil || len(got) != 0 {
			t.Fatalf("CheckAll(%v) = %#v", rs, got)
		}
	}
}

func TestCheckAllMatchesSeparateChecks(t *testing.T) {
	src := "policy u * /api/** 2/1m by=user\npolicy w POST /** 3/1m algo=window\n"
	rs := []engine.Request{
		req("POST", "/api/a", "ann", "1"), req("GET", "/api/b", "ann", "2"), req("POST", "/api/c", "bob", "1"),
		req("POST", "/api/d", "ann", "1"), req("POST", "/z", "", "1"), req("POST", "/z", "", "1"),
	}
	e1, _ := newEngine(t, src)
	e2, _ := newEngine(t, src)
	all := e1.CheckAll(rs)
	for i, r := range rs {
		if d := e2.Check(r); !reflect.DeepEqual(d, all[i]) {
			t.Fatalf("request %d: CheckAll %+v, Check %+v", i+1, all[i], d)
		}
	}
}
