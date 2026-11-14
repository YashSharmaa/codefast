package policy_test

import (
	"testing"

	"example.com/throttle/policy"
)

type pathCase struct {
	pattern, path string
	want          bool
}

func checkPaths(t *testing.T, cases []pathCase) {
	t.Helper()
	for _, c := range cases {
		if got := policy.MatchPath(c.pattern, c.path); got != c.want {
			t.Errorf("MatchPath(%q, %q) = %v, want %v", c.pattern, c.path, got, c.want)
		}
	}
}

func TestMatchPathDropsQueryAndFragment(t *testing.T) {
	checkPaths(t, []pathCase{
		{"/login", "/login?next=/home", true},
		{"/login", "/login#top", true},
		{"/login", "/login?", true},
		{"/login", "/login#a?b", true},
		{"/login", "/login/?x", false},
		{"/login/", "/login/?x", true},
		{"/api/**", "/api?x", true},
	})
}

func TestMatchPathQueryNeverSplitsSegments(t *testing.T) {
	checkPaths(t, []pathCase{
		{"/api/*", "/api/v1?x=/a/b", true},
		{"/api/*/*", "/api/v1?x=/a/b", false},
		{"/api/*", "/api/?x=1", false},
		{"/*", "/#/deep/link", false},
		{"/", "/#/deep/link", true},
	})
}

func TestMatchPathEmptyAfterCutMatchesNothing(t *testing.T) {
	checkPaths(t, []pathCase{
		{"/**", "?x=/a", false},
		{"/**", "#/", false},
		{"/**", "", false},
		{"/**", "/?x", true},
	})
}

func TestMatchPathDecodesEscapes(t *testing.T) {
	checkPaths(t, []pathCase{
		{"/login", "/log%69n", true},
		{"/login", "/%6Cogin", true},
		{"/login", "/%6cogin", true},
		{"/Login", "/%4cogin", true},
		{"/login", "/%4cogin", false},
		{"/a b", "/a%20b", true},
		{"/api/users", "/api%2Fusers", false},
	})
}

func TestMatchPathKeepsEscapedSlashesAndBadEscapes(t *testing.T) {
	checkPaths(t, []pathCase{
		{"/*", "/a%2Fb", true},
		{"/a%2Fb", "/a%2Fb", true},
		{"/a%2fb", "/a%2fb", true},
		{"/a/b", "/a%2fb", false},
		{"/100%", "/100%", true},
		{"/a%zz", "/a%zz", true},
		{"/a%4", "/a%4", true},
		{"/%41", "/%2541", true},
		{"/A", "/%2541", false},
	})
}

func TestMatchPathCutsBeforeDecoding(t *testing.T) {
	checkPaths(t, []pathCase{
		{"/login", "/login%3Fnext", false},
		{"/*", "/login%3Fnext", true},
		{"/a/*", "/a/%23x", true},
		{"/a", "/a%23x", false},
	})
}

func TestParsePatternWithQueryOrFragmentIsBad(t *testing.T) {
	expectMsg(t, []string{
		"policy a * /login?x 1/1s",
		"policy a * /a# 1/1s",
		"policy a * /? 1/1s",
		"policy a * /x/*/y#z 1/1s",
	}, "bad pattern")
	expectMsg(t, []string{"policy a get /a? 1/1s"}, "bad methods")
	if ps := mustParse(t, "policy a * /100% 1/1s\n"); ps[0].Pattern != "/100%" {
		t.Fatalf("pattern = %q", ps[0].Pattern)
	}
}

func TestPolicyMatchesCleansThePath(t *testing.T) {
	ps := mustParse(t, "policy w POST,PUT /items/* 1/1s\n")
	if !ps[0].Matches("post", "/items/9?x=1") || !ps[0].Matches("PUT", "/items/%39") {
		t.Fatalf("Matches must clean the path")
	}
	if ps[0].Matches("POST", "/items/9%2F1") != true || ps[0].Matches("POST", "/items/9/1") {
		t.Fatalf("an escaped slash stays inside its segment")
	}
}
