package plan_test

import (
	"errors"
	"reflect"
	"testing"

	"example.com/buildplan/plan"
)

const patterned = `CC = cc
app: main.o util.o
	$(CC) -o $@ $^
%.o: %.c defs.h
	$(CC) -c $< -o $@
`

func TestPlanTargetsFromRules(t *testing.T) {
	st := plan.MapState{"main.c": 1, "util.c": 7, "defs.h": 1, "main.o": 5, "util.o": 5, "app": 9}
	steps := mustMake(t, patterned, nil, st)
	want := []brief{{"util.o", "dependency util.c is newer"}, {"app", "dependency util.o is being rebuilt"}}
	if got := summary(steps); !reflect.DeepEqual(got, want) {
		t.Fatalf("steps = %v, want %v", got, want)
	}
	if !reflect.DeepEqual(steps[0].Commands, []string{"cc -c util.c -o util.o"}) || !reflect.DeepEqual(steps[1].Commands, []string{"cc -o app main.o util.o"}) {
		t.Fatalf("commands = %q / %q", steps[0].Commands, steps[1].Commands)
	}
	steps = mustMake(t, patterned, nil, plan.MapState{"main.c": 1, "util.c": 1, "defs.h": 1})
	if got := summary(steps); !reflect.DeepEqual(got, []brief{{"main.o", "missing"}, {"util.o", "missing"}, {"app", "missing"}}) {
		t.Fatalf("steps = %v", got)
	}
}

func TestPlanMissingNeededByRuleTarget(t *testing.T) {
	_, err := plan.Make(file(t, patterned), nil, plan.MapState{"main.c": 1, "defs.h": 1})
	var me *plan.MissingError
	if !errors.As(err, &me) || me.Name != "util.c" || me.NeededBy != "util.o" {
		t.Fatalf("got %v (%T)", err, err)
	}
}

func TestPlanRulesDoNotApplyToGoals(t *testing.T) {
	f := file(t, patterned)
	_, err := plan.Make(f, []string{"extra.o"}, plan.MapState{"extra.c": 1, "defs.h": 1})
	var me *plan.MissingError
	if !errors.As(err, &me) || me.Name != "extra.o" || me.NeededBy != "" {
		t.Fatalf("got %v (%T)", err, err)
	}
	steps, err := plan.Make(f, []string{"extra.o"}, plan.MapState{"extra.o": 1})
	if err != nil || len(steps) != 0 {
		t.Fatalf("steps=%v err=%v", summary(steps), err)
	}
}
