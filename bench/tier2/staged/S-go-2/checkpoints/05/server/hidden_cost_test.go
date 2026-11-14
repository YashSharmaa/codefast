package server_test

import "testing"

func TestServerPoliciesShowCost(t *testing.T) {
	h, _ := handler(t, "policy upload POST /upload 10/1m cost=5\npolicy any * /** 2/1s\n")
	want := `[{"name":"upload","methods":"POST","pattern":"/upload","limit":10,"window_seconds":60,"burst":10,"by":"ip","algo":"bucket","cost":5},` +
		`{"name":"any","methods":"*","pattern":"/**","limit":2,"window_seconds":1,"burst":2,"by":"ip","algo":"bucket","cost":1}]` + "\n"
	expect(t, do(h, "GET", "/policies", ""), 200, want)
}

func TestServerCheckWithCost(t *testing.T) {
	h, _ := handler(t, "policy upload POST /upload 10/1m cost=5\n")
	body := `{"method":"POST","path":"/upload","ip":"1.2.3.4"}`
	expect(t, do(h, "POST", "/check", body), 200, `{"allowed":true,"matched":["upload"],"denied":"","retry_after":0,"remaining":5}`+"\n")
	do(h, "POST", "/check", body)
	w := do(h, "POST", "/check", body)
	expect(t, w, 429, `{"allowed":false,"matched":["upload"],"denied":"upload","retry_after":30,"remaining":0}`+"\n")
	if got := w.Header().Get("Retry-After"); got != "30" {
		t.Fatalf("Retry-After = %q", got)
	}
}
