package replay_test

import (
	"errors"
	"reflect"
	"testing"

	"example.com/throttle/engine"
	"example.com/throttle/policy"
	"example.com/throttle/replay"
)

func policies(t *testing.T, src string) []policy.Policy {
	t.Helper()
	ps, err := policy.Parse(src)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	return ps
}

func run(t *testing.T, src, log string) *replay.Report {
	t.Helper()
	rep, err := replay.Run(policies(t, src), log)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	return rep
}

func lineErr(t *testing.T, log string) *replay.LineError {
	t.Helper()
	rep, err := replay.Run(policies(t, "policy p * /** 1/1s\n"), log)
	var le *replay.LineError
	if !errors.As(err, &le) {
		t.Fatalf("log %q: expected *replay.LineError, got %v (%T)", log, err, err)
	}
	if rep != nil {
		t.Fatalf("log %q: report must be nil on an error", log)
	}
	return le
}

func TestReplayCountsAndResults(t *testing.T) {
	rep := run(t, "policy api * /api/** 2/1m\n", "0 GET /api/a ip=1\n0 GET /api/b ip=1\n1000 GET /api/c ip=1\n2000 GET /other ip=1\n")
	if rep.Requests != 4 || rep.Allowed != 3 || rep.Denied != 1 {
		t.Fatalf("counts = %d %d %d", rep.Requests, rep.Allowed, rep.Denied)
	}
	if !reflect.DeepEqual(rep.Policies, []replay.Summary{{Name: "api", Matched: 3, Denied: 1}}) {
		t.Fatalf("Policies = %+v", rep.Policies)
	}
	want := []replay.Result{
		{Line: 1, Decision: engine.Decision{Allowed: true, Matched: []string{"api"}, Remaining: 1}},
		{Line: 2, Decision: engine.Decision{Allowed: true, Matched: []string{"api"}, Remaining: 0}},
		{Line: 3, Decision: engine.Decision{Allowed: false, Matched: []string{"api"}, Denied: "api", RetryAfter: 29, Remaining: 0}},
		{Line: 4, Decision: engine.Decision{Allowed: true, Matched: []string{}, Remaining: -1}},
	}
	if !reflect.DeepEqual(rep.Results, want) {
		t.Fatalf("Results = %+v\nwant %+v", rep.Results, want)
	}
}

func TestReplayClockFollowsOffsets(t *testing.T) {
	rep := run(t, "policy p * /** 1/10s\n", "0 GET / ip=a\n5000 GET / ip=a\n9999 GET / ip=a\n10000 GET / ip=a\n")
	var got []bool
	for _, r := range rep.Results {
		got = append(got, r.Decision.Allowed)
	}
	if !reflect.DeepEqual(got, []bool{true, false, false, true}) {
		t.Fatalf("allowed = %v", got)
	}
	if rep.Results[1].Decision.RetryAfter != 5 || rep.Results[2].Decision.RetryAfter != 1 {
		t.Fatalf("retry = %+v", rep.Results)
	}
}

func TestReplaySameOffsetIsSameTime(t *testing.T) {
	rep := run(t, "policy w * /** 2/1s algo=window\n", "500 GET / ip=a\n500 GET / ip=a\n500 GET / ip=a\n1499 GET / ip=a\n1500 GET / ip=a\n")
	var got []bool
	for _, r := range rep.Results {
		got = append(got, r.Decision.Allowed)
	}
	if !reflect.DeepEqual(got, []bool{true, true, false, false, true}) {
		t.Fatalf("allowed = %v", got)
	}
}

func TestReplayUserAndIPOptions(t *testing.T) {
	src := "policy u * /** 1/1h by=user\npolicy i * /ip 1/1h\n"
	log := "0 GET / user=alice ip=1\n1 GET / ip=2 user=alice\n2 GET /\n3 GET / user= ip=\n4 GET /ip ip=\n5 GET /ip\n"
	rep := run(t, src, log)
	var got []bool
	for _, r := range rep.Results {
		got = append(got, r.Decision.Allowed)
	}
	if !reflect.DeepEqual(got, []bool{true, false, true, false, false, false}) {
		t.Fatalf("allowed = %v", got)
	}
	if !reflect.DeepEqual(rep.Policies, []replay.Summary{{"u", 6, 4}, {"i", 2, 0}}) {
		t.Fatalf("Policies = %+v", rep.Policies)
	}
}

func TestReplayCommentsAndLineNumbers(t *testing.T) {
	rep := run(t, "policy p * /** 5/1s\n", "# morning traffic\n\n  0 get /a\n   # more\n7  POST  /b   ip=x\n")
	if len(rep.Results) != 2 || rep.Results[0].Line != 3 || rep.Results[1].Line != 5 {
		t.Fatalf("Results = %+v", rep.Results)
	}
	if !rep.Results[0].Decision.Allowed || !reflect.DeepEqual(rep.Results[0].Decision.Matched, []string{"p"}) {
		t.Fatalf("lower-case method must match: %+v", rep.Results[0])
	}
}

func TestReplayBadLines(t *testing.T) {
	for _, log := range []string{
		"0 GET\n",
		"0 GET / user=a ip=b extra\n",
		"0 GET / who=x\n",
		"0 GET / user=a user=b\n",
		"0 GET / ip\n",
		"0 GET / USER=a\n",
		"x GET / who=x\n",
	} {
		le := lineErr(t, "# header\n"+log)
		if le.Line != 2 || le.Msg != "bad line" {
			t.Fatalf("%q: got %+v", log, le)
		}
	}
}

func TestReplayBadOffsets(t *testing.T) {
	for _, log := range []string{"-1 GET /\n", "+5 GET /\n", "1.5 GET /\n", "1e3 GET /\n", "soon GET /\n", "1_000 GET /\n"} {
		le := lineErr(t, "0 GET /\n"+log)
		if le.Line != 2 || le.Msg != "bad offset" {
			t.Fatalf("%q: got %+v", log, le)
		}
	}
	le := lineErr(t, "0 GET /\n100 GET /\n\n100 GET /\n99 GET /\n200 bad\n")
	if le.Line != 5 || le.Msg != "offset goes back" || le.Error() != "line 5: offset goes back" {
		t.Fatalf("got %+v / %q", le, le.Error())
	}
}

func TestReplayEmptyLogAndUnmatchedPolicies(t *testing.T) {
	rep := run(t, "policy a * /a 1/1s\npolicy b * /b 1/1s\n", "# nothing\n\n")
	if rep.Requests != 0 || rep.Allowed != 0 || rep.Denied != 0 {
		t.Fatalf("counts = %+v", rep)
	}
	if rep.Results == nil || len(rep.Results) != 0 {
		t.Fatalf("Results = %#v", rep.Results)
	}
	if !reflect.DeepEqual(rep.Policies, []replay.Summary{{"a", 0, 0}, {"b", 0, 0}}) {
		t.Fatalf("Policies = %+v", rep.Policies)
	}
	empty, err := replay.Run(nil, "0 GET /x\n")
	if err != nil || empty.Policies == nil || len(empty.Policies) != 0 || empty.Allowed != 1 {
		t.Fatalf("no policies: %+v %v", empty, err)
	}
}

func TestReplayDeniedCountsOnlyTheNamedPolicy(t *testing.T) {
	src := "policy first * /x 1/1m\npolicy second * /x 1/1m\npolicy wide * /** 10/1s\n"
	rep := run(t, src, "0 GET /x\n0 GET /x\n0 GET /x\n0 GET /y\n")
	want := []replay.Summary{{"first", 3, 2}, {"second", 3, 0}, {"wide", 4, 0}}
	if !reflect.DeepEqual(rep.Policies, want) {
		t.Fatalf("Policies = %+v, want %+v", rep.Policies, want)
	}
	again := run(t, src, "0 GET /x\n0 GET /x\n0 GET /x\n0 GET /y\n")
	if !reflect.DeepEqual(again, rep) {
		t.Fatalf("every run starts with fresh buckets")
	}
}
