package replay_test

import (
	"reflect"
	"testing"

	"example.com/throttle/replay"
)

func TestReplayCleansPaths(t *testing.T) {
	rep := run(t, "policy p * /a/* 2/1m\n", "0 GET /a/b?page=2\n1 GET /a/%62\n2 GET /a/b#x\n3 GET /a%2Fb\n")
	var got []bool
	for _, r := range rep.Results {
		got = append(got, r.Decision.Allowed)
	}
	if !reflect.DeepEqual(got, []bool{true, true, false, true}) {
		t.Fatalf("allowed = %v", got)
	}
	if !reflect.DeepEqual(rep.Policies, []replay.Summary{{"p", 3, 1}}) {
		t.Fatalf("Policies = %+v", rep.Policies)
	}
}
