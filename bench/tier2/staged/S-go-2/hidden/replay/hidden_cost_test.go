package replay_test

import (
	"reflect"
	"testing"
)

func TestReplayUsesCosts(t *testing.T) {
	rep := run(t, "policy up POST /up 10/1m cost=4\npolicy any * /** 100/1m\n", "0 POST /up\n0 POST /up\n0 POST /up\n15000 POST /up\n")
	var got []bool
	for _, r := range rep.Results {
		got = append(got, r.Decision.Allowed)
	}
	if !reflect.DeepEqual(got, []bool{true, true, false, true}) {
		t.Fatalf("allowed = %v", got)
	}
	if rep.Results[2].Decision.RetryAfter != 12 || rep.Results[3].Decision.Remaining != 0 {
		t.Fatalf("results = %+v", rep.Results)
	}
}
