package plan_test

import (
	"errors"
	"reflect"
	"testing"

	"example.com/buildplan/plan"
)

func TestPlanGoalRuleMissingDependency(t *testing.T) {
	_, err := plan.Make(file(t, patterned), []string{"extra.o"}, plan.MapState{"defs.h": 1})
	var me *plan.MissingError
	if !errors.As(err, &me) || me.Name != "extra.c" || me.NeededBy != "extra.o" {
		t.Fatalf("got %v (%T)", err, err)
	}
	if me.Error() != `missing "extra.c" needed by "extra.o"` {
		t.Fatalf("Error() = %q", me.Error())
	}
}

func TestPlanGoalRuleReasons(t *testing.T) {
	f := file(t, patterned)
	st := plan.MapState{"extra.c": 3, "defs.h": 1, "extra.o": 5}
	steps, err := plan.Make(f, []string{"extra.o"}, st)
	if err != nil || len(steps) != 0 {
		t.Fatalf("up to date goal: steps=%v err=%v", summary(steps), err)
	}
	st["defs.h"] = 8
	steps = mustMake(t, patterned, []string{"extra.o", "main.o"}, plan.MapState{"extra.c": 3, "defs.h": 8, "extra.o": 5, "main.c": 1})
	want := []brief{{"extra.o", "dependency defs.h is newer"}, {"main.o", "missing"}}
	if got := summary(steps); !reflect.DeepEqual(got, want) {
		t.Fatalf("steps = %v, want %v", got, want)
	}
	if !reflect.DeepEqual(steps[0].Commands, []string{"cc -c extra.c -o extra.o"}) {
		t.Fatalf("commands = %q", steps[0].Commands)
	}
}
