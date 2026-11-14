package engine_test

import (
	"reflect"
	"testing"
	"time"

	"example.com/throttle/clock"
	"example.com/throttle/engine"
	"example.com/throttle/policy"
)

var t0 = time.Date(2026, 3, 1, 12, 0, 0, 0, time.UTC)

func newEngine(t *testing.T, src string) (*engine.Engine, *clock.Fake) {
	t.Helper()
	ps, err := policy.Parse(src)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	c := clock.NewFake(t0)
	return engine.New(ps, c), c
}

func req(method, path, user, ip string) engine.Request {
	return engine.Request{Method: method, Path: path, User: user, IP: ip}
}

func TestEngineBurstThenDeny(t *testing.T) {
	e, _ := newEngine(t, "policy api * /api/** 3/1m\n")
	r := req("GET", "/api/x", "", "10.0.0.1")
	for i, wantLeft := range []int{2, 1, 0} {
		d := e.Check(r)
		want := engine.Decision{Allowed: true, Matched: []string{"api"}, Denied: "", RetryAfter: 0, Remaining: wantLeft}
		if !reflect.DeepEqual(d, want) {
			t.Fatalf("request %d: got %+v, want %+v", i+1, d, want)
		}
	}
	d := e.Check(r)
	want := engine.Decision{Allowed: false, Matched: []string{"api"}, Denied: "api", RetryAfter: 20, Remaining: 0}
	if !reflect.DeepEqual(d, want) {
		t.Fatalf("got %+v, want %+v", d, want)
	}
}

func TestEngineDenyTakesNoTokens(t *testing.T) {
	e, _ := newEngine(t, "policy all * /api/** 5/1m\npolicy upload POST /api/upload 1/1m\n")
	d := e.Check(req("POST", "/api/upload", "", "ip"))
	if !d.Allowed || d.Remaining != 0 || !reflect.DeepEqual(d.Matched, []string{"all", "upload"}) {
		t.Fatalf("first upload: %+v", d)
	}
	d = e.Check(req("POST", "/api/upload", "", "ip"))
	if d.Allowed || d.Denied != "upload" || d.RetryAfter != 60 || d.Remaining != 0 {
		t.Fatalf("second upload: %+v", d)
	}
	d = e.Check(req("GET", "/api/list", "", "ip"))
	if !d.Allowed || d.Remaining != 3 || !reflect.DeepEqual(d.Matched, []string{"all"}) {
		t.Fatalf("the denied upload must not have taken a token from all: %+v", d)
	}
}

func TestEngineDeniedFirstInFileOrderRetryIsLargest(t *testing.T) {
	e, _ := newEngine(t, "policy fast * /x 1/1m\npolicy slow * /x 1/1h\npolicy wide * /** 100/1s\n")
	if d := e.Check(req("GET", "/x", "", "a")); !d.Allowed {
		t.Fatalf("first: %+v", d)
	}
	d := e.Check(req("GET", "/x", "", "a"))
	want := engine.Decision{Allowed: false, Matched: []string{"fast", "slow", "wide"}, Denied: "fast", RetryAfter: 3600, Remaining: 0}
	if !reflect.DeepEqual(d, want) {
		t.Fatalf("got %+v, want %+v", d, want)
	}
}

func TestEngineKeysByIP(t *testing.T) {
	e, _ := newEngine(t, "policy p * /** 1/1m by=ip\n")
	if !e.Check(req("GET", "/", "u", "1.1.1.1")).Allowed || !e.Check(req("GET", "/", "u", "2.2.2.2")).Allowed {
		t.Fatalf("each IP has its own bucket")
	}
	if e.Check(req("GET", "/", "other", "1.1.1.1")).Allowed {
		t.Fatalf("same IP, different user must share the bucket")
	}
}

func TestEngineKeysByUserWithAnonymous(t *testing.T) {
	e, _ := newEngine(t, "policy p * /** 1/1m by=user\n")
	if !e.Check(req("GET", "/", "alice", "1")).Allowed {
		t.Fatalf("alice first")
	}
	if e.Check(req("GET", "/", "alice", "2")).Allowed {
		t.Fatalf("alice from another IP must share her bucket")
	}
	if !e.Check(req("GET", "/", "", "3")).Allowed {
		t.Fatalf("empty user first")
	}
	d := e.Check(req("GET", "/", "anonymous", "4"))
	if d.Allowed || d.Denied != "p" {
		t.Fatalf("empty user counts as anonymous: %+v", d)
	}
}

func TestEngineGlobalKeyShared(t *testing.T) {
	e, _ := newEngine(t, "policy g * /** 2/1m by=global burst=2\n")
	e.Check(req("GET", "/a", "x", "1"))
	e.Check(req("PUT", "/b", "y", "2"))
	d := e.Check(req("GET", "/c", "z", "3"))
	if d.Allowed || d.Denied != "g" || d.RetryAfter != 30 {
		t.Fatalf("global bucket must be shared: %+v", d)
	}
}

func TestEngineNoMatchingPolicy(t *testing.T) {
	e, _ := newEngine(t, "policy p GET /only 1/1m\n")
	d := e.Check(req("POST", "/only", "", "1"))
	if !d.Allowed || d.Matched == nil || len(d.Matched) != 0 || d.Denied != "" || d.RetryAfter != 0 || d.Remaining != -1 {
		t.Fatalf("got %+v", d)
	}
}

func TestEngineRefillsWithClock(t *testing.T) {
	var _ clock.Clock = clock.Real{}
	e, c := newEngine(t, "policy p * /** 2/10s\n")
	if !c.Now().Equal(t0) {
		t.Fatalf("fake clock Now = %v", c.Now())
	}
	r := req("get", "/z", "", "9")
	e.Check(r)
	e.Check(r)
	d := e.Check(r)
	if d.Allowed || d.RetryAfter != 5 {
		t.Fatalf("got %+v", d)
	}
	c.Advance(4999 * time.Millisecond)
	if d = e.Check(r); d.Allowed || d.RetryAfter != 1 {
		t.Fatalf("at 4.999s: %+v", d)
	}
	c.Advance(time.Millisecond)
	if !c.Now().Equal(t0.Add(5 * time.Second)) {
		t.Fatalf("fake clock Now = %v", c.Now())
	}
	if d = e.Check(r); !d.Allowed || d.Remaining != 0 {
		t.Fatalf("at 5s: %+v", d)
	}
}

func TestEngineBucketCreatedAtFirstSight(t *testing.T) {
	e, c := newEngine(t, "policy p * /** 1/1m\n")
	e.Check(req("GET", "/", "", "old"))
	c.Advance(30 * time.Second)
	if d := e.Check(req("GET", "/", "", "old")); d.Allowed || d.RetryAfter != 30 {
		t.Fatalf("old key: %+v", d)
	}
	if d := e.Check(req("GET", "/", "", "new")); !d.Allowed {
		t.Fatalf("new key starts full: %+v", d)
	}
	if d := e.Check(req("GET", "/", "", "new")); d.Allowed || d.RetryAfter != 60 {
		t.Fatalf("new key: %+v", d)
	}
}

func TestEngineRemainingIsMinimumAfterTaking(t *testing.T) {
	e, _ := newEngine(t, "policy big * /** 10/1m\npolicy small * /s/* 3/1m by=user\n")
	d := e.Check(req("GET", "/s/1", "bob", "1"))
	if !d.Allowed || d.Remaining != 2 {
		t.Fatalf("got %+v", d)
	}
	d = e.Check(req("GET", "/t", "bob", "1"))
	if !d.Allowed || d.Remaining != 8 {
		t.Fatalf("got %+v", d)
	}
	ps := e.Policies()
	if len(ps) != 2 || ps[0].Name != "big" || ps[1].Name != "small" {
		t.Fatalf("Policies = %+v", ps)
	}
}
