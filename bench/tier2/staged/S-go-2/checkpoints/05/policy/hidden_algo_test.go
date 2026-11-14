package policy_test

import (
	"reflect"
	"testing"
	"time"

	"example.com/throttle/policy"
)

func TestParseAlgoOption(t *testing.T) {
	ps := mustParse(t, "policy q * /q/** 100/1m algo=window by=user\npolicy b * / 5/1s algo=bucket burst=8 by=ip\npolicy d * / 1/1h\n")
	want := policy.Policy{Name: "q", Methods: nil, Pattern: "/q/**", Limit: 100, Window: time.Minute, Burst: 100, By: "user", Algo: "window", Cost: 1, Line: 1}
	if !reflect.DeepEqual(ps[0], want) {
		t.Fatalf("got %+v\nwant %+v", ps[0], want)
	}
	if ps[1].Algo != "bucket" || ps[1].Burst != 8 {
		t.Fatalf("b = %+v", ps[1])
	}
	if ps[2].Algo != "bucket" {
		t.Fatalf("default algo = %q", ps[2].Algo)
	}
}

func TestParseAlgoErrors(t *testing.T) {
	expectMsg(t, []string{
		"policy a * / 5/1m algo=fixed",
		"policy a * / 5/1m algo=",
		"policy a * / 5/1m algo=Window",
		"policy a * / 5/1m algo=window algo=window",
		"policy a * / 5/1m algo=window burst=5",
		"policy a * / 5/1m burst=2 algo=window",
	}, "bad option")
	expectMsg(t, []string{"policy a * / 5/1m algo=bucket burst=5 by=ip cost=1 extra"}, "bad line")
	ps := mustParse(t, "policy a * / 5/1m algo=window by=global\npolicy b * / 5/1m algo=bucket burst=5 by=ip\n")
	if len(ps) != 2 || ps[0].By != "global" || ps[0].Burst != 5 {
		t.Fatalf("got %+v", ps)
	}
}
