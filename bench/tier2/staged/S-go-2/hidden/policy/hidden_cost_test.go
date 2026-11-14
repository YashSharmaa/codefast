package policy_test

import (
	"reflect"
	"testing"
	"time"

	"example.com/throttle/policy"
)

func TestParseCostOption(t *testing.T) {
	ps := mustParse(t, "policy upload POST /upload 10/1m cost=5\npolicy all * /** 3/1s burst=9 by=user cost=9\npolicy plain * / 2/1h\n")
	want := policy.Policy{Name: "upload", Methods: []string{"POST"}, Pattern: "/upload", Limit: 10, Window: time.Minute, Burst: 10, By: "ip", Algo: "bucket", Cost: 5, Line: 1}
	if !reflect.DeepEqual(ps[0], want) {
		t.Fatalf("got %+v\nwant %+v", ps[0], want)
	}
	if ps[1].Cost != 9 || ps[1].Burst != 9 || ps[1].By != "user" {
		t.Fatalf("all = %+v", ps[1])
	}
	if ps[2].Cost != 1 {
		t.Fatalf("default cost = %d, want 1", ps[2].Cost)
	}
}

func TestParseCostAgainstBurst(t *testing.T) {
	ps := mustParse(t, "policy a * / 4/1m cost=4\npolicy b * / 1/1m cost=3 burst=3\npolicy c * / 9/1m burst=2 cost=2\npolicy w * / 4/1m cost=4 algo=window\n")
	if ps[0].Cost != 4 || ps[1].Cost != 3 || ps[1].Burst != 3 || ps[2].Cost != 2 || ps[3].Cost != 4 || ps[3].Algo != "window" {
		t.Fatalf("got %+v", ps)
	}
	expectMsg(t, []string{
		"policy a * / 5/1m cost=6",
		"policy a * / 10/1m cost=3 burst=2",
		"policy a * / 10/1m burst=2 cost=3",
		"policy a * / 4/1m algo=window cost=5",
	}, "bad option")
}

func TestParseCostBadValues(t *testing.T) {
	expectMsg(t, []string{
		"policy a * / 5/1m cost=0",
		"policy a * / 5/1m cost=x",
		"policy a * / 5/1m cost=+1",
		"policy a * / 5/1m cost=",
		"policy a * / 5/1m cost=1 cost=1",
		"policy a * / 5/1m cost=1 by=ip cost=2",
	}, "bad option")
	expectMsg(t, []string{"policy a * / 5/1m burst=5 by=ip algo=bucket cost=1 extra"}, "bad line")
	if ps := mustParse(t, "policy a * / 5/1m burst=5 by=ip algo=bucket cost=1\n"); len(ps) != 1 {
		t.Fatalf("nine fields are allowed")
	}
}
