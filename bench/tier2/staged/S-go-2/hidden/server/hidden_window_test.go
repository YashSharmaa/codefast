package server_test

import (
	"testing"
	"time"
)

func TestServerPoliciesShowAlgo(t *testing.T) {
	h, _ := handler(t, "policy q GET /q 100/1m algo=window\npolicy b * /** 2/1s\n")
	want := `[{"name":"q","methods":"GET","pattern":"/q","limit":100,"window_seconds":60,"burst":100,"by":"ip","algo":"window","cost":1},` +
		`{"name":"b","methods":"*","pattern":"/**","limit":2,"window_seconds":1,"burst":2,"by":"ip","algo":"bucket","cost":1}]` + "\n"
	expect(t, do(h, "GET", "/policies", ""), 200, want)
}

func TestServerWindowCheck(t *testing.T) {
	h, c := handler(t, "policy q * /q 2/30s algo=window\n")
	body := `{"method":"GET","path":"/q","ip":"1.1.1.1"}`
	do(h, "POST", "/check", body)
	expect(t, do(h, "POST", "/check", body), 200, `{"allowed":true,"matched":["q"],"denied":"","retry_after":0,"remaining":0}`+"\n")
	c.Advance(12500 * time.Millisecond)
	w := do(h, "POST", "/check", body)
	expect(t, w, 429, `{"allowed":false,"matched":["q"],"denied":"q","retry_after":18,"remaining":0}`+"\n")
	if got := w.Header().Get("Retry-After"); got != "18" {
		t.Fatalf("Retry-After = %q", got)
	}
}
