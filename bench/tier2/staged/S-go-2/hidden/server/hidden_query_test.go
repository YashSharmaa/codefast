package server_test

import "testing"

func TestServerQueryStringBugReport(t *testing.T) {
	h, _ := handler(t, "policy login * /login 5/30s\n")
	expect(t, do(h, "POST", "/check", `{"method":"POST","path":"/login?next=/home","ip":"1.2.3.4"}`), 200,
		`{"allowed":true,"matched":["login"],"denied":"","retry_after":0,"remaining":4}`+"\n")
	expect(t, do(h, "POST", "/check", `{"method":"POST","path":"/log%69n","ip":"1.2.3.4"}`), 200,
		`{"allowed":true,"matched":["login"],"denied":"","retry_after":0,"remaining":3}`+"\n")
	expect(t, do(h, "POST", "/check", `{"method":"POST","path":"?only","ip":"1.2.3.4"}`), 200,
		`{"allowed":true,"matched":[],"denied":"","retry_after":0,"remaining":-1}`+"\n")
}
