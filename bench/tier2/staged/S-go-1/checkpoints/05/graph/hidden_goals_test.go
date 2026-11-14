package graph_test

import (
	"reflect"
	"testing"

	"example.com/buildplan/graph"
	"example.com/buildplan/parse"
)

func withGoals(t *testing.T, src string, goals ...string) *graph.Graph {
	t.Helper()
	f, err := parse.Parse(src)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	return graph.FromFile(f, goals)
}

const objects = "app: x.o\n%.o: %.c defs.h\n%.c: %.y\n"

func TestFromFileGoalsGetRules(t *testing.T) {
	g := withGoals(t, objects, "y.o")
	if got := g.Nodes(); !reflect.DeepEqual(got, []string{"app", "x.o", "x.c", "defs.h", "y.o", "y.c"}) {
		t.Fatalf("Nodes = %q", got)
	}
	if got := g.Deps("y.o"); !reflect.DeepEqual(got, []string{"y.c", "defs.h"}) {
		t.Fatalf("Deps(y.o) = %q", got)
	}
	if got := g.Deps("y.c"); len(got) != 0 {
		t.Fatalf("dependencies of a goal's target are not matched: %q", got)
	}
}

func TestFromFileNilAndEmptyGoals(t *testing.T) {
	want := []string{"app", "x.o", "x.c", "defs.h"}
	for _, g := range []*graph.Graph{withGoals(t, objects), withGoals(t, objects, []string{}...)} {
		if got := g.Nodes(); !reflect.DeepEqual(got, want) {
			t.Fatalf("Nodes = %q", got)
		}
	}
}

func TestFromFileUnmatchedGoalIsNotAdded(t *testing.T) {
	g := withGoals(t, objects, "ghost", "app")
	if got := g.Nodes(); !reflect.DeepEqual(got, []string{"app", "x.o", "x.c", "defs.h"}) {
		t.Fatalf("Nodes = %q", got)
	}
	got, err := g.Order([]string{"ghost"})
	if err != nil || !reflect.DeepEqual(got, []string{"ghost"}) {
		t.Fatalf("Order = %q, %v", got, err)
	}
}

func TestFromFileGoalThatIsARuleDependency(t *testing.T) {
	g := withGoals(t, objects, "x.c", "x.o", "x.c")
	if got := g.Nodes(); !reflect.DeepEqual(got, []string{"app", "x.o", "x.c", "defs.h", "x.y"}) {
		t.Fatalf("Nodes = %q", got)
	}
	if got := g.Deps("x.c"); !reflect.DeepEqual(got, []string{"x.y"}) {
		t.Fatalf("Deps(x.c) = %q", got)
	}
	if got := g.Deps("x.o"); !reflect.DeepEqual(got, []string{"x.c", "defs.h"}) {
		t.Fatalf("Deps(x.o) = %q", got)
	}
}

func TestFromFileGoalsInGivenOrder(t *testing.T) {
	g := withGoals(t, "%.o: %.c\n", "b.o", "a.o", "b.o")
	if got := g.Nodes(); !reflect.DeepEqual(got, []string{"b.o", "b.c", "a.o", "a.c"}) {
		t.Fatalf("Nodes = %q", got)
	}
	got, err := g.Order([]string{"a.o", "b.o"})
	if err != nil || !reflect.DeepEqual(got, []string{"a.c", "a.o", "b.c", "b.o"}) {
		t.Fatalf("Order = %q, %v", got, err)
	}
}
