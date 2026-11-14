package graph_test

import (
	"errors"
	"reflect"
	"testing"

	"example.com/buildplan/graph"
	"example.com/buildplan/parse"
)

func fromSrc(t *testing.T, src string) *graph.Graph {
	t.Helper()
	f, err := parse.Parse(src)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	return graph.FromFile(f, nil)
}

func cycleErr(t *testing.T, err error) *graph.CycleError {
	t.Helper()
	var ce *graph.CycleError
	if !errors.As(err, &ce) {
		t.Fatalf("expected *graph.CycleError, got %v (%T)", err, err)
	}
	return ce
}

func TestGraphAddEdgeNodesAndDeps(t *testing.T) {
	g := graph.New()
	g.AddNode("z")
	g.AddEdge("a", "b")
	g.AddEdge("a", "c")
	g.AddEdge("a", "b")
	g.AddEdge("c", "z")
	g.AddNode("a")
	if got := g.Nodes(); !reflect.DeepEqual(got, []string{"z", "a", "b", "c"}) {
		t.Fatalf("Nodes = %q", got)
	}
	if got := g.Deps("a"); !reflect.DeepEqual(got, []string{"b", "c"}) {
		t.Fatalf("Deps(a) = %q", got)
	}
	if got := g.Deps("nope"); len(got) != 0 {
		t.Fatalf("Deps(nope) = %q", got)
	}
}

func TestGraphFromFileNodeOrder(t *testing.T) {
	g := fromSrc(t, "all: prog docs\nprog: main.o lib.o\ndocs: README\nmain.o: main.c\n")
	want := []string{"all", "prog", "docs", "main.o", "lib.o", "README", "main.c"}
	if got := g.Nodes(); !reflect.DeepEqual(got, want) {
		t.Fatalf("Nodes = %q, want %q", got, want)
	}
	if got := g.Deps("prog"); !reflect.DeepEqual(got, []string{"main.o", "lib.o"}) {
		t.Fatalf("Deps(prog) = %q", got)
	}
}

func TestOrderDependenciesFirstDiamond(t *testing.T) {
	g := fromSrc(t, "top: left right\nleft: base x\nright: base y\nbase: core\n")
	got, err := g.Order([]string{"top"})
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"core", "base", "x", "left", "y", "right", "top"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("Order = %q, want %q", got, want)
	}
}

func TestOrderSeveralGoalsInGivenOrder(t *testing.T) {
	g := fromSrc(t, "a: b c\nb: d\nc: d\ne: d f\n")
	got, err := g.Order([]string{"e", "a", "e", "b"})
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"d", "f", "e", "b", "c", "a"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("Order = %q, want %q", got, want)
	}
}

func TestOrderUnknownGoalIsLeaf(t *testing.T) {
	g := fromSrc(t, "a: b\n")
	got, err := g.Order([]string{"ghost", "a"})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, []string{"ghost", "b", "a"}) {
		t.Fatalf("Order = %q", got)
	}
}

func TestOrderCyclePathStartsAtRepeatedNode(t *testing.T) {
	g := fromSrc(t, "a: b\nb: c\nc: b\n")
	order, err := g.Order([]string{"a"})
	if order != nil {
		t.Fatalf("order must be nil on a cycle, got %q", order)
	}
	ce := cycleErr(t, err)
	if !reflect.DeepEqual(ce.Path, []string{"b", "c", "b"}) {
		t.Fatalf("Path = %q", ce.Path)
	}
	if ce.Error() != "cycle: b -> c -> b" {
		t.Fatalf("Error() = %q", ce.Error())
	}
}

func TestOrderCycleFoundAfterFinishedBranch(t *testing.T) {
	g := fromSrc(t, "a: b\nb: c d\nc:\nd: e\ne: c b\n")
	_, err := g.Order([]string{"a"})
	ce := cycleErr(t, err)
	if !reflect.DeepEqual(ce.Path, []string{"b", "d", "e", "b"}) {
		t.Fatalf("Path = %q", ce.Path)
	}
}

func TestOrderSelfLoop(t *testing.T) {
	g := fromSrc(t, "ok: x\nx: x\n")
	_, err := g.Order([]string{"ok"})
	ce := cycleErr(t, err)
	if !reflect.DeepEqual(ce.Path, []string{"x", "x"}) || ce.Error() != "cycle: x -> x" {
		t.Fatalf("got %q / %q", ce.Path, ce.Error())
	}
}

func TestOrderIgnoresUnreachedCycle(t *testing.T) {
	g := fromSrc(t, "p: q\nq: p\nfine: leaf\n")
	got, err := g.Order([]string{"fine"})
	if err != nil {
		t.Fatalf("unexpected error %v", err)
	}
	if !reflect.DeepEqual(got, []string{"leaf", "fine"}) {
		t.Fatalf("Order = %q", got)
	}
}

func TestCyclesComponentsSorted(t *testing.T) {
	g := fromSrc(t, "z: y\ny: x\nx: z w\nw:\nm: m\nb: a\na: c\nc: b\nfree: z\n")
	got := g.Cycles()
	want := [][]string{{"a", "b", "c"}, {"m"}, {"x", "y", "z"}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("Cycles = %q, want %q", got, want)
	}
	if n := len(fromSrc(t, "a: b c\nb: c\n").Cycles()); n != 0 {
		t.Fatalf("acyclic graph gave %d cycles", n)
	}
}
