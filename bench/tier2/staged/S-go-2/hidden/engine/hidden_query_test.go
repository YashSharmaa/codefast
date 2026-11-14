package engine_test

import "testing"

func TestEngineQueryStringSharesTheBucket(t *testing.T) {
	e, _ := newEngine(t, "policy login * /login 1/1m\n")
	if d := e.Check(req("POST", "/login", "", "1.2.3.4")); !d.Allowed {
		t.Fatalf("first: %+v", d)
	}
	for _, p := range []string{"/login?next=/home", "/log%69n", "/login#x"} {
		if d := e.Check(req("POST", p, "", "1.2.3.4")); d.Allowed || d.Denied != "login" || d.RetryAfter != 60 {
			t.Fatalf("%s: %+v", p, d)
		}
	}
}
