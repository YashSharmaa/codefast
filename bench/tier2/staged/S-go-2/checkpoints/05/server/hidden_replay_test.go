package server_test

import "testing"

func TestServerReplay(t *testing.T) {
	h, _ := handler(t, policies)
	log := "0 GET /api/x user=ann\n0 POST /api/y user=ann\n10 GET /api/z user=ann\n20 GET /login\n"
	want := `{"requests":4,"allowed":3,"denied":1,` +
		`"policies":[{"name":"api","matched":3,"denied":1},{"name":"login","matched":1,"denied":0}],` +
		`"results":[{"line":1,"allowed":true,"denied":"","retry_after":0,"remaining":1},` +
		`{"line":2,"allowed":true,"denied":"","retry_after":0,"remaining":0},` +
		`{"line":3,"allowed":false,"denied":"api","retry_after":6,"remaining":0},` +
		`{"line":4,"allowed":true,"denied":"","retry_after":0,"remaining":4}]}` + "\n"
	expect(t, do(h, "POST", "/replay", log), 200, want)
	empty, _ := handler(t, "# none\n")
	expect(t, do(empty, "POST", "/replay", ""), 200, `{"requests":0,"allowed":0,"denied":0,"policies":[],"results":[]}`+"\n")
}

func TestServerReplayErrorsAndMethods(t *testing.T) {
	h, _ := handler(t, policies)
	expect(t, do(h, "POST", "/replay", "0 GET /a\n\n5 GET /b\n3 GET /c\n"), 400, `{"error":"line 4: offset goes back"}`+"\n")
	expect(t, do(h, "POST", "/replay", "now GET /a\n"), 400, `{"error":"line 1: bad offset"}`+"\n")
	w := do(h, "GET", "/replay", "")
	expect(t, w, 405, `{"error":"method not allowed"}`+"\n")
	if w.Header().Get("Allow") != "POST" {
		t.Fatalf("Allow = %q", w.Header().Get("Allow"))
	}
	expect(t, do(h, "POST", "/replay/", ""), 404, `{"error":"not found"}`+"\n")
}

func TestServerReplayLeavesLiveBucketsAlone(t *testing.T) {
	h, _ := handler(t, policies)
	do(h, "POST", "/replay", "0 GET /api/x user=ann\n0 GET /api/x user=ann\n0 GET /api/x user=ann\n")
	w := do(h, "POST", "/check", `{"method":"GET","path":"/api/x","user":"ann"}`)
	expect(t, w, 200, `{"allowed":true,"matched":["api"],"denied":"","retry_after":0,"remaining":1}`+"\n")
}
