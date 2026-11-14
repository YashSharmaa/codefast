package policy_test

import (
	"errors"
	"reflect"
	"testing"
	"time"

	"example.com/throttle/policy"
)

func mustParse(t *testing.T, src string) []policy.Policy {
	t.Helper()
	ps, err := policy.Parse(src)
	if err != nil {
		t.Fatalf("Parse: unexpected error %v", err)
	}
	return ps
}

func parseErr(t *testing.T, src string) *policy.ParseError {
	t.Helper()
	_, err := policy.Parse(src)
	if err == nil {
		t.Fatalf("Parse(%q): expected an error", src)
	}
	var pe *policy.ParseError
	if !errors.As(err, &pe) {
		t.Fatalf("error %v (%T) is not *policy.ParseError", err, err)
	}
	return pe
}

func expectMsg(t *testing.T, lines []string, msg string) {
	t.Helper()
	for _, l := range lines {
		pe := parseErr(t, "# first\n"+l+"\n")
		if pe.Line != 2 || pe.Msg != msg {
			t.Fatalf("%q: got line %d %q, want line 2 %q", l, pe.Line, pe.Msg, msg)
		}
	}
}

func TestParsePolicyFieldsAndDefaults(t *testing.T) {
	src := "# limits\n\npolicy api GET,POST /api/** 10/1m burst=20 by=user\n   # indented comment\npolicy login * /login 5/30s\n"
	ps := mustParse(t, src)
	if len(ps) != 2 {
		t.Fatalf("got %d policies", len(ps))
	}
	want0 := policy.Policy{Name: "api", Methods: []string{"GET", "POST"}, Pattern: "/api/**", Limit: 10, Window: time.Minute, Burst: 20, By: "user", Algo: "bucket", Line: 3}
	if !reflect.DeepEqual(ps[0], want0) {
		t.Fatalf("got %+v\nwant %+v", ps[0], want0)
	}
	want1 := policy.Policy{Name: "login", Methods: nil, Pattern: "/login", Limit: 5, Window: 30 * time.Second, Burst: 5, By: "ip", Algo: "bucket", Line: 5}
	if !reflect.DeepEqual(ps[1], want1) {
		t.Fatalf("got %+v\nwant %+v", ps[1], want1)
	}
}

func TestParseWindowsAndOptionOrder(t *testing.T) {
	ps := mustParse(t, "policy a * / 1/45s\npolicy b-2 * / 7/3m by=global burst=2\npolicy c_3 DELETE /x 100/2h by=ip\n")
	if ps[0].Window != 45*time.Second || ps[1].Window != 3*time.Minute || ps[2].Window != 2*time.Hour {
		t.Fatalf("windows = %v %v %v", ps[0].Window, ps[1].Window, ps[2].Window)
	}
	if ps[1].By != "global" || ps[1].Burst != 2 || ps[1].Limit != 7 {
		t.Fatalf("b-2 = %+v", ps[1])
	}
	if ps[2].By != "ip" || ps[2].Burst != 100 || !reflect.DeepEqual(ps[2].Methods, []string{"DELETE"}) {
		t.Fatalf("c_3 = %+v", ps[2])
	}
}

func TestParseBadLine(t *testing.T) {
	expectMsg(t, []string{
		"rule a * / 1/1s",
		"policy a * /",
		"policy a * / 1/1s burst=2 by=ip algo=bucket extra",
		"Policy a * / 1/1s",
	}, "bad line")
}

func TestParseBadName(t *testing.T) {
	expectMsg(t, []string{"policy api! * / 1/1s", "policy a.b * / 1/1s"}, "bad name")
}

func TestParseDuplicateName(t *testing.T) {
	pe := parseErr(t, "policy a * /x 1/1s\npolicy b * /y 1/1s\n\npolicy a * /z 2/1s\n")
	if pe.Line != 4 || pe.Msg != `duplicate policy "a"` {
		t.Fatalf("got %+v", pe)
	}
	if pe.Error() != `line 4: duplicate policy "a"` {
		t.Fatalf("Error() = %q", pe.Error())
	}
}

func TestParseBadMethods(t *testing.T) {
	expectMsg(t, []string{
		"policy a get / 1/1s",
		"policy a GET, / 1/1s",
		"policy a GET,* / 1/1s",
		"policy a ** / 1/1s",
	}, "bad methods")
}

func TestParseBadPattern(t *testing.T) {
	expectMsg(t, []string{
		"policy a * api/x 1/1s",
		"policy a * /a/**/b 1/1s",
		"policy a * /v* 1/1s",
		"policy a * /*** 1/1s",
	}, "bad pattern")
}

func TestParseBadRate(t *testing.T) {
	expectMsg(t, []string{
		"policy a * / 10",
		"policy a * / 0/1m",
		"policy a * / 10/0s",
		"policy a * / 10/1d",
		"policy a * / 10/m",
		"policy a * / +5/1m",
		"policy a * / 10/1m/2",
		"policy a * / 10/1.5m",
	}, "bad rate")
}

func TestParseBadOption(t *testing.T) {
	expectMsg(t, []string{
		"policy a * / 1/1s burst=0",
		"policy a * / 1/1s by=host",
		"policy a * / 1/1s burst=2 burst=3",
		"policy a * / 1/1s by=ip by=ip",
		"policy a * / 1/1s limit=4",
		"policy a * / 1/1s burst",
		"policy a * / 1/1s burst=-1",
	}, "bad option")
}

func TestParseFirstFailingCheckWins(t *testing.T) {
	expectMsg(t, []string{"policy a! get bad 0/1x by=no"}, "bad name")
	expectMsg(t, []string{"policy a get bad 0/1x by=no"}, "bad methods")
	expectMsg(t, []string{"policy a * bad 0/1x by=no"}, "bad pattern")
	expectMsg(t, []string{"policy a * /ok 0/1x by=no"}, "bad rate")
	pe := parseErr(t, "policy a * / 1/1s\npolicy a * / 1/1s\npolicy c * / 1/1x\n")
	if pe.Line != 2 {
		t.Fatalf("parsing must stop at the first bad line, got %+v", pe)
	}
}

func TestMatchPath(t *testing.T) {
	cases := []struct {
		pattern, path string
		want          bool
	}{
		{"/api/**", "/api", true},
		{"/api/**", "/api/", true},
		{"/api/**", "/api/v1/users", true},
		{"/api/**", "/apix", false},
		{"/api/**", "/", false},
		{"/**", "/", true},
		{"/**", "/anything/at/all", true},
		{"/users/*", "/users/42", true},
		{"/users/*", "/users/", false},
		{"/users/*", "/users", false},
		{"/users/*", "/users/42/posts", false},
		{"/users/*/posts", "/users/7/posts", true},
		{"/users/*/posts", "/users/7/Posts", false},
		{"/", "/", true},
		{"/", "/a", false},
		{"/a/", "/a", false},
		{"/a/", "/a/", true},
		{"/login", "login", false},
		{"/*", "/x", true},
		{"/*/**", "/", false},
		{"/*/**", "/x", true},
	}
	for _, c := range cases {
		if got := policy.MatchPath(c.pattern, c.path); got != c.want {
			t.Errorf("MatchPath(%q, %q) = %v, want %v", c.pattern, c.path, got, c.want)
		}
	}
}

func TestPolicyMatchesMethod(t *testing.T) {
	ps := mustParse(t, "policy w POST,PUT /items/* 1/1s\npolicy any * /items/** 1/1s\n")
	if !ps[0].Matches("post", "/items/9") || !ps[0].Matches("PUT", "/items/9") {
		t.Fatalf("method comparison must upper-case the request method")
	}
	if ps[0].Matches("GET", "/items/9") || ps[0].Matches("POST", "/items") {
		t.Fatalf("w must not match GET or a wrong path")
	}
	if !ps[1].Matches("PATCH", "/items") || !ps[1].Matches("whatever", "/items/a/b") {
		t.Fatalf("* must match any method")
	}
}
