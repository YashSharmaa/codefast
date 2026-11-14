package plan_test

import (
	"reflect"
	"testing"

	"example.com/buildplan/plan"
)

func TestPlanPhonyTargetsAlwaysRun(t *testing.T) {
	src := ".PHONY: all clean\nall: app\napp: main.c\n\tcc -o app main.c\nclean:\n\trm -f app\ndist: clean app\n\ttar app\n"
	st := plan.MapState{"main.c": 1, "app": 5, "all": 99, "clean": 99, "dist": 99}
	steps := mustMake(t, src, nil, st)
	if got := summary(steps); !reflect.DeepEqual(got, []brief{{"all", "phony"}}) {
		t.Fatalf("steps = %v", got)
	}
	steps = mustMake(t, src, []string{"dist"}, st)
	want := []brief{{"clean", "phony"}, {"dist", "dependency clean is being rebuilt"}}
	if got := summary(steps); !reflect.DeepEqual(got, want) {
		t.Fatalf("steps = %v, want %v", got, want)
	}
	if !reflect.DeepEqual(steps[0].Commands, []string{"rm -f app"}) {
		t.Fatalf("commands = %q", steps[0].Commands)
	}
	steps = mustMake(t, src, []string{"all"}, plan.MapState{"main.c": 1})
	if got := summary(steps); !reflect.DeepEqual(got, []brief{{"app", "missing"}, {"all", "phony"}}) {
		t.Fatalf("steps = %v", got)
	}
}
