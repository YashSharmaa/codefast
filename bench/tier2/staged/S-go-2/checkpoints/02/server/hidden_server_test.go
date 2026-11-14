package server_test

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"example.com/throttle/clock"
	"example.com/throttle/engine"
	"example.com/throttle/policy"
	"example.com/throttle/server"
)

const policies = "# test policies\npolicy api GET,POST /api/** 10/1m burst=2 by=user\npolicy login * /login 5/30s\n"

func handler(t *testing.T, src string) (http.Handler, *clock.Fake) {
	t.Helper()
	ps, err := policy.Parse(src)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	c := clock.NewFake(time.Date(2026, 3, 1, 12, 0, 0, 0, time.UTC))
	return server.NewHandler(engine.New(ps, c)), c
}

func do(h http.Handler, method, path, body string) *httptest.ResponseRecorder {
	r := httptest.NewRequest(method, path, strings.NewReader(body))
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

func expect(t *testing.T, w *httptest.ResponseRecorder, status int, body string) {
	t.Helper()
	if w.Code != status {
		t.Fatalf("status = %d, want %d (body %q)", w.Code, status, w.Body.String())
	}
	if got := w.Body.String(); got != body {
		t.Fatalf("body = %q, want %q", got, body)
	}
	if ct := w.Header().Get("Content-Type"); ct != "application/json" {
		t.Fatalf("Content-Type = %q", ct)
	}
}

func TestServerCheckAllowed(t *testing.T) {
	h, _ := handler(t, policies)
	w := do(h, "POST", "/check", `{"method":"GET","path":"/api/items","user":"ann","ip":"1.2.3.4"}`)
	expect(t, w, 200, `{"allowed":true,"matched":["api"],"denied":"","retry_after":0,"remaining":1}`+"\n")
	if w.Header().Get("Retry-After") != "" {
		t.Fatalf("Retry-After must only be set on 429")
	}
}

func TestServerCheckDeniedHasRetryAfter(t *testing.T) {
	h, _ := handler(t, policies)
	body := `{"method":"POST","path":"/api/x","user":"ann","ip":"1.2.3.4"}`
	do(h, "POST", "/check", body)
	do(h, "POST", "/check", body)
	w := do(h, "POST", "/check", body)
	expect(t, w, 429, `{"allowed":false,"matched":["api"],"denied":"api","retry_after":6,"remaining":0}`+"\n")
	if got := w.Header().Get("Retry-After"); got != "6" {
		t.Fatalf("Retry-After = %q", got)
	}
}

func TestServerCheckRefillsWithClock(t *testing.T) {
	h, c := handler(t, policies)
	body := `{"method":"DELETE","path":"/login","ip":"9.9.9.9"}`
	for i := 0; i < 5; i++ {
		if w := do(h, "POST", "/check", body); w.Code != 200 {
			t.Fatalf("request %d: %d", i+1, w.Code)
		}
	}
	if w := do(h, "POST", "/check", body); w.Code != 429 || w.Header().Get("Retry-After") != "6" {
		t.Fatalf("got %d %q", w.Code, w.Header().Get("Retry-After"))
	}
	c.Advance(6 * time.Second)
	w := do(h, "POST", "/check", body)
	expect(t, w, 200, `{"allowed":true,"matched":["login"],"denied":"","retry_after":0,"remaining":0}`+"\n")
}

func TestServerCheckNoMatchAndExtraFields(t *testing.T) {
	h, _ := handler(t, policies)
	w := do(h, "POST", "/check", `{"method":"PUT","path":"/api/x","extra":[1,2],"user":"z"}`)
	expect(t, w, 200, `{"allowed":true,"matched":[],"denied":"","retry_after":0,"remaining":-1}`+"\n")
}

func TestServerCheckBadRequests(t *testing.T) {
	h, _ := handler(t, policies)
	for _, body := range []string{
		``,
		`{`,
		`not json`,
		`[]`,
		`null`,
		`{"method":5,"path":"/x"}`,
		`{"method":"GET"}`,
		`{"method":"","path":"/x"}`,
		`{"method":"GET","path":"/x","user":true}`,
	} {
		w := do(h, "POST", "/check", body)
		if w.Code != 400 || w.Body.String() != `{"error":"bad request"}`+"\n" {
			t.Fatalf("body %q: got %d %q", body, w.Code, w.Body.String())
		}
	}
}

func TestServerPolicies(t *testing.T) {
	h, _ := handler(t, policies)
	w := do(h, "GET", "/policies", "")
	want := `[{"name":"api","methods":"GET,POST","pattern":"/api/**","limit":10,"window_seconds":60,"burst":2,"by":"user","algo":"bucket"},` +
		`{"name":"login","methods":"*","pattern":"/login","limit":5,"window_seconds":30,"burst":5,"by":"ip","algo":"bucket"}]` + "\n"
	expect(t, w, 200, want)
	empty, _ := handler(t, "# nothing here\n")
	expect(t, do(empty, "GET", "/policies", ""), 200, "[]\n")
}

func TestServerMethodNotAllowed(t *testing.T) {
	h, _ := handler(t, policies)
	w := do(h, "GET", "/check", "")
	expect(t, w, 405, `{"error":"method not allowed"}`+"\n")
	if w.Header().Get("Allow") != "POST" {
		t.Fatalf("Allow = %q", w.Header().Get("Allow"))
	}
	w = do(h, "DELETE", "/policies", "")
	expect(t, w, 405, `{"error":"method not allowed"}`+"\n")
	if w.Header().Get("Allow") != "GET" {
		t.Fatalf("Allow = %q", w.Header().Get("Allow"))
	}
}

func TestServerNotFound(t *testing.T) {
	h, _ := handler(t, policies)
	for _, p := range []string{"/", "/check/", "/policy", "/checks"} {
		expect(t, do(h, "POST", p, "{}"), 404, `{"error":"not found"}`+"\n")
	}
	expect(t, do(h, "GET", "/nope", ""), 404, `{"error":"not found"}`+"\n")
}
