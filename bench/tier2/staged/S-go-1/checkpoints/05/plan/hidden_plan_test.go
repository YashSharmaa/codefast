package plan_test

import (
	"errors"
	"reflect"
	"testing"

	"example.com/buildplan/graph"
	"example.com/buildplan/parse"
	"example.com/buildplan/plan"
)

const project = `CC = cc
app: main.o util.o
	$(CC) -o app main.o util.o
main.o: main.c defs.h
	$(CC) -c main.c
util.o: util.c defs.h
	$(CC) -c util.c
`

func file(t *testing.T, src string) *parse.File {
	t.Helper()
	f, err := parse.Parse(src)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	return f
}

type brief struct{ target, reason string }

func summary(steps []plan.Step) []brief {
	out := []brief{}
	for _, s := range steps {
		out = append(out, brief{s.Target, s.Reason})
	}
	return out
}

func mustMake(t *testing.T, src string, goals []string, st plan.State) []plan.Step {
	t.Helper()
	steps, err := plan.Make(file(t, src), goals, st)
	if err != nil {
		t.Fatalf("Make: unexpected error %v", err)
	}
	return steps
}

func TestPlanEverythingMissing(t *testing.T) {
	st := plan.MapState{"main.c": 1, "util.c": 1, "defs.h": 1}
	steps := mustMake(t, project, nil, st)
	want := []brief{{"main.o", "missing"}, {"util.o", "missing"}, {"app", "missing"}}
	if got := summary(steps); !reflect.DeepEqual(got, want) {
		t.Fatalf("steps = %v, want %v", got, want)
	}
	if !reflect.DeepEqual(steps[0].Commands, []string{"cc -c main.c"}) || !reflect.DeepEqual(steps[2].Commands, []string{"cc -o app main.o util.o"}) {
		t.Fatalf("commands = %q / %q", steps[0].Commands, steps[2].Commands)
	}
}

func TestPlanUpToDateGivesNoSteps(t *testing.T) {
	st := plan.MapState{"main.c": 1, "util.c": 1, "defs.h": 1, "main.o": 5, "util.o": 5, "app": 9}
	steps := mustMake(t, project, []string{"app"}, st)
	if len(steps) != 0 {
		t.Fatalf("expected no steps, got %v", summary(steps))
	}
}

func TestPlanNewerDependencyPropagates(t *testing.T) {
	st := plan.MapState{"main.c": 1, "util.c": 7, "defs.h": 1, "main.o": 5, "util.o": 5, "app": 9}
	steps := mustMake(t, project, nil, st)
	want := []brief{{"util.o", "dependency util.c is newer"}, {"app", "dependency util.o is being rebuilt"}}
	if got := summary(steps); !reflect.DeepEqual(got, want) {
		t.Fatalf("steps = %v, want %v", got, want)
	}
}

func TestPlanFirstNewerDependencyInDeclaredOrder(t *testing.T) {
	st := plan.MapState{"main.c": 6, "util.c": 1, "defs.h": 8, "main.o": 5, "util.o": 5, "app": 9}
	steps := mustMake(t, project, []string{"main.o"}, st)
	want := []brief{{"main.o", "dependency main.c is newer"}}
	if got := summary(steps); !reflect.DeepEqual(got, want) {
		t.Fatalf("steps = %v, want %v", got, want)
	}
}

func TestPlanRebuiltRuleBeforeNewerRule(t *testing.T) {
	src := "out: input gen\n\tbuild\ngen: tool\n\tmake gen\n"
	st := plan.MapState{"input": 50, "tool": 1, "out": 10}
	steps := mustMake(t, src, nil, st)
	want := []brief{{"gen", "missing"}, {"out", "dependency gen is being rebuilt"}}
	if got := summary(steps); !reflect.DeepEqual(got, want) {
		t.Fatalf("steps = %v, want %v", got, want)
	}
}

func TestPlanEqualTimesAreNotNewer(t *testing.T) {
	src := "b: a\n\tcp a b\n"
	if steps := mustMake(t, src, nil, plan.MapState{"a": 4, "b": 4}); len(steps) != 0 {
		t.Fatalf("equal times must not rebuild: %v", summary(steps))
	}
	steps := mustMake(t, src, nil, plan.MapState{"a": 5, "b": 4})
	if got := summary(steps); !reflect.DeepEqual(got, []brief{{"b", "dependency a is newer"}}) {
		t.Fatalf("steps = %v", got)
	}
}

func TestPlanMissingSourceFile(t *testing.T) {
	st := plan.MapState{"main.c": 1, "util.c": 1}
	_, err := plan.Make(file(t, project), nil, st)
	var me *plan.MissingError
	if !errors.As(err, &me) {
		t.Fatalf("expected *plan.MissingError, got %v (%T)", err, err)
	}
	if me.Name != "defs.h" || me.NeededBy != "main.o" {
		t.Fatalf("got %+v", me)
	}
	if me.Error() != `missing "defs.h" needed by "main.o"` {
		t.Fatalf("Error() = %q", me.Error())
	}
}

func TestPlanUndeclaredGoals(t *testing.T) {
	f := file(t, "a: b\n\tmk\n")
	_, err := plan.Make(f, []string{"ghost"}, plan.MapState{"b": 1})
	var me *plan.MissingError
	if !errors.As(err, &me) || me.Name != "ghost" || me.NeededBy != "" || me.Error() != `missing "ghost"` {
		t.Fatalf("got %v (%T)", err, err)
	}
	steps, err := plan.Make(f, []string{"b"}, plan.MapState{"b": 1})
	if err != nil || len(steps) != 0 {
		t.Fatalf("existing plain file goal: steps=%v err=%v", summary(steps), err)
	}
}

func TestPlanDefaultGoalAndNoTargets(t *testing.T) {
	f := file(t, "first: x\n\tone\nsecond:\n\ttwo\n")
	steps, err := plan.Make(f, nil, plan.MapState{"x": 1})
	if err != nil {
		t.Fatal(err)
	}
	if got := summary(steps); !reflect.DeepEqual(got, []brief{{"first", "missing"}}) {
		t.Fatalf("steps = %v", got)
	}
	goals, err := plan.DefaultGoals(f, []string{"second", "first"})
	if err != nil || !reflect.DeepEqual(goals, []string{"second", "first"}) {
		t.Fatalf("DefaultGoals = %q, %v", goals, err)
	}
	_, err = plan.Make(file(t, "# nothing\nX = 1\n"), nil, plan.MapState{})
	if !errors.Is(err, plan.ErrNoTargets) {
		t.Fatalf("expected ErrNoTargets, got %v", err)
	}
}

func TestPlanCycleAndCommandlessTargets(t *testing.T) {
	_, err := plan.Make(file(t, "a: b\nb: a\n"), nil, plan.MapState{"a": 1, "b": 1})
	var ce *graph.CycleError
	if !errors.As(err, &ce) || !reflect.DeepEqual(ce.Path, []string{"a", "b", "a"}) {
		t.Fatalf("expected cycle a b a, got %v", err)
	}
	steps := mustMake(t, "all: one two\none:\n\techo 1\ntwo:\n", nil, plan.MapState{})
	want := []brief{{"one", "missing"}, {"two", "missing"}, {"all", "missing"}}
	if got := summary(steps); !reflect.DeepEqual(got, want) {
		t.Fatalf("steps = %v", got)
	}
	if len(steps[1].Commands) != 0 || len(steps[2].Commands) != 0 {
		t.Fatalf("commandless targets must have no commands")
	}
}
