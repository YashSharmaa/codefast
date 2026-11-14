package engine_test

import (
	"reflect"
	"testing"
	"time"

	"example.com/throttle/engine"
)

func TestEngineCostDeniesWhenNotEnough(t *testing.T) {
	e, c := newEngine(t, "policy upload POST /upload 10/1m cost=4\n")
	r := req("POST", "/upload", "", "1.1.1.1")
	for i, left := range []int{6, 2} {
		d := e.Check(r)
		if !d.Allowed || d.Remaining != left {
			t.Fatalf("upload %d: %+v", i+1, d)
		}
	}
	d := e.Check(r)
	want := engine.Decision{Allowed: false, Matched: []string{"upload"}, Denied: "upload", RetryAfter: 12, Remaining: 2}
	if !reflect.DeepEqual(d, want) {
		t.Fatalf("got %+v, want %+v", d, want)
	}
	c.Advance(12 * time.Second)
	if d := e.Check(r); !d.Allowed || d.Remaining != 0 {
		t.Fatalf("after 12s: %+v", d)
	}
}

func TestEngineCostTakenFromEachMatchingPolicy(t *testing.T) {
	e, _ := newEngine(t, "policy all * /** 10/1m\npolicy up POST /up 5/1m cost=5\n")
	d := e.Check(req("POST", "/up", "", "ip"))
	if !d.Allowed || d.Remaining != 0 || !reflect.DeepEqual(d.Matched, []string{"all", "up"}) {
		t.Fatalf("first upload: %+v", d)
	}
	d = e.Check(req("POST", "/up", "", "ip"))
	if d.Allowed || d.Denied != "up" || d.RetryAfter != 60 {
		t.Fatalf("second upload: %+v", d)
	}
	if d = e.Check(req("GET", "/x", "", "ip")); !d.Allowed || d.Remaining != 8 {
		t.Fatalf("all must have lost one token (cost 1) and nothing for the denied upload: %+v", d)
	}
}

func TestEngineRetryAfterUsesEachCost(t *testing.T) {
	e, _ := newEngine(t, "policy a * /x 6/1m cost=4\npolicy b * /x 60/1m burst=6 cost=6\n")
	if d := e.Check(req("GET", "/x", "", "k")); !d.Allowed || d.Remaining != 0 {
		t.Fatalf("first: %+v", d)
	}
	d := e.Check(req("GET", "/x", "", "k"))
	want := engine.Decision{Allowed: false, Matched: []string{"a", "b"}, Denied: "a", RetryAfter: 20, Remaining: 0}
	if !reflect.DeepEqual(d, want) {
		t.Fatalf("got %+v, want %+v", d, want)
	}
}
