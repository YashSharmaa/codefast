package server_test

import "testing"

const one = "policy one * /x 1/1m\npolicy two * /y 2/30s\n"

func TestServerBatchExample(t *testing.T) {
	h, _ := handler(t, one)
	w := do(h, "POST", "/check", `[{"method":"GET","path":"/x"},{"path":"/x"},{"method":"GET","path":"/x"}]`)
	expect(t, w, 429, `[{"allowed":true,"matched":["one"],"denied":"","retry_after":0,"remaining":0},{"error":"bad request"},`+
		`{"allowed":false,"matched":["one"],"denied":"one","retry_after":60,"remaining":0}]`+"\n")
	if got := w.Header().Get("Retry-After"); got != "60" {
		t.Fatalf("Retry-After = %q", got)
	}
}

func TestServerBatchAllAllowed(t *testing.T) {
	h, _ := handler(t, one)
	w := do(h, "POST", "/check", ` [ {"method":"GET","path":"/y","ip":"1"}, {"method":"GET","path":"/z","ip":"1"} ] `)
	expect(t, w, 200, `[{"allowed":true,"matched":["two"],"denied":"","retry_after":0,"remaining":1},{"allowed":true,"matched":[],"denied":"","retry_after":0,"remaining":-1}]`+"\n")
	if w.Header().Get("Retry-After") != "" {
		t.Fatalf("Retry-After must only be set on 429")
	}
}

func TestServerBatchBadElements(t *testing.T) {
	h, _ := handler(t, one)
	body := `[5, null, [], "x", {"method":5,"path":"/x"}, {"method":"GET"}, {"method":"","path":"/x"}, {"method":"GET","path":"/x","ip":true}, {"method":"GET","path":"/x"}]`
	bad := `{"error":"bad request"},`
	want := "[" + bad + bad + bad + bad + bad + bad + bad + bad + `{"allowed":true,"matched":["one"],"denied":"","retry_after":0,"remaining":0}]` + "\n"
	expect(t, do(h, "POST", "/check", body), 200, want)
	expect(t, do(h, "POST", "/check", `[{}, 1]`), 200, `[{"error":"bad request"},{"error":"bad request"}]`+"\n")
}

func TestServerBatchEmpty(t *testing.T) {
	h, _ := handler(t, one)
	expect(t, do(h, "POST", "/check", `[]`), 200, "[]\n")
	expect(t, do(h, "POST", "/check", " [ ]\n"), 200, "[]\n")
}

func TestServerBatchRetryAfterIsLargest(t *testing.T) {
	h, _ := handler(t, one)
	do(h, "POST", "/check", `{"method":"GET","path":"/x"}`)
	w := do(h, "POST", "/check", `[{"method":"GET","path":"/x"},{"method":"GET","path":"/y"},{"method":"GET","path":"/y"},{"method":"GET","path":"/y"}]`)
	expect(t, w, 429, `[{"allowed":false,"matched":["one"],"denied":"one","retry_after":60,"remaining":0},`+
		`{"allowed":true,"matched":["two"],"denied":"","retry_after":0,"remaining":1},`+
		`{"allowed":true,"matched":["two"],"denied":"","retry_after":0,"remaining":0},`+
		`{"allowed":false,"matched":["two"],"denied":"two","retry_after":15,"remaining":0}]`+"\n")
	if got := w.Header().Get("Retry-After"); got != "60" {
		t.Fatalf("Retry-After = %q", got)
	}
}

func TestServerBadBodiesStillRejected(t *testing.T) {
	h, _ := handler(t, one)
	for _, body := range []string{`null`, `"x"`, `5`, `true`, `[1,`, `[] []`, `{} {}`, `[{"method":"GET","path":"/x"}] x`} {
		w := do(h, "POST", "/check", body)
		if w.Code != 400 || w.Body.String() != `{"error":"bad request"}`+"\n" {
			t.Fatalf("body %q: got %d %q", body, w.Code, w.Body.String())
		}
	}
	expect(t, do(h, "POST", "/check", `{"method":"GET","path":"/x"}`), 200, `{"allowed":true,"matched":["one"],"denied":"","retry_after":0,"remaining":0}`+"\n")
}
