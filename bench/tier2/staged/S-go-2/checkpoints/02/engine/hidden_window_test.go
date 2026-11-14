package engine_test

import (
	"reflect"
	"testing"
	"time"

	"example.com/throttle/engine"
)

func TestEngineWindowPolicy(t *testing.T) {
	e, c := newEngine(t, "policy w * /** 3/10s algo=window\n")
	r := req("GET", "/a", "", "1")
	for i, left := range []int{2, 1, 0} {
		if d := e.Check(r); !d.Allowed || d.Remaining != left {
			t.Fatalf("request %d: %+v", i+1, d)
		}
	}
	d := e.Check(r)
	want := engine.Decision{Allowed: false, Matched: []string{"w"}, Denied: "w", RetryAfter: 10, Remaining: 0}
	if !reflect.DeepEqual(d, want) {
		t.Fatalf("got %+v, want %+v", d, want)
	}
	c.Advance(4 * time.Second)
	if d = e.Check(r); d.Allowed || d.RetryAfter != 6 {
		t.Fatalf("at 4s: %+v", d)
	}
	c.Advance(6 * time.Second)
	if d = e.Check(r); !d.Allowed || d.Remaining != 2 {
		t.Fatalf("at 10s: %+v", d)
	}
}

func TestEngineWindowPerKeyFromFirstSight(t *testing.T) {
	e, c := newEngine(t, "policy w * /** 1/10s algo=window\n")
	e.Check(req("GET", "/", "", "a"))
	c.Advance(6 * time.Second)
	e.Check(req("GET", "/", "", "b"))
	c.Advance(4 * time.Second)
	if d := e.Check(req("GET", "/", "", "a")); !d.Allowed {
		t.Fatalf("a's window started at 0s and ended at 10s: %+v", d)
	}
	if d := e.Check(req("GET", "/", "", "b")); d.Allowed || d.RetryAfter != 6 {
		t.Fatalf("b's window started at 6s: %+v", d)
	}
}

func TestEngineMixedAlgorithms(t *testing.T) {
	e, _ := newEngine(t, "policy b * /** 10/1m\npolicy w POST /up 2/1m algo=window\n")
	for i := 0; i < 2; i++ {
		if d := e.Check(req("POST", "/up", "", "1")); !d.Allowed || !reflect.DeepEqual(d.Matched, []string{"b", "w"}) {
			t.Fatalf("upload %d: %+v", i+1, d)
		}
	}
	d := e.Check(req("POST", "/up", "", "1"))
	want := engine.Decision{Allowed: false, Matched: []string{"b", "w"}, Denied: "w", RetryAfter: 60, Remaining: 0}
	if !reflect.DeepEqual(d, want) {
		t.Fatalf("got %+v, want %+v", d, want)
	}
	if d = e.Check(req("GET", "/x", "", "1")); !d.Allowed || d.Remaining != 7 {
		t.Fatalf("b must have lost two tokens only: %+v", d)
	}
}
