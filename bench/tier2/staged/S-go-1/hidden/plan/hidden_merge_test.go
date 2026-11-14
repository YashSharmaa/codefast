package plan_test

import (
	"reflect"
	"testing"

	"example.com/buildplan/plan"
)

func TestPlanSeesMergedDependencies(t *testing.T) {
	src := "app: main.o\n\tlink app\nmain.o: main.c\n\tcc main\napp: config.h\n"
	st := plan.MapState{"main.c": 1, "main.o": 2, "app": 5, "config.h": 9}
	steps := mustMake(t, src, nil, st)
	if got := summary(steps); !reflect.DeepEqual(got, []brief{{"app", "dependency config.h is newer"}}) {
		t.Fatalf("steps = %v", got)
	}
	if !reflect.DeepEqual(steps[0].Commands, []string{"link app"}) {
		t.Fatalf("commands = %q", steps[0].Commands)
	}
}
