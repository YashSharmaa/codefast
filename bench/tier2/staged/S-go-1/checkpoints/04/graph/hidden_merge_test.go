package graph_test

import (
	"reflect"
	"testing"
)

func TestGraphMergedTargetEdges(t *testing.T) {
	g := fromSrc(t, "a: x\nb: y\na: z x\n")
	if got := g.Nodes(); !reflect.DeepEqual(got, []string{"a", "x", "z", "b", "y"}) {
		t.Fatalf("Nodes = %q", got)
	}
	if got := g.Deps("a"); !reflect.DeepEqual(got, []string{"x", "z"}) {
		t.Fatalf("Deps(a) = %q", got)
	}
	got, err := g.Order([]string{"a"})
	if err != nil || !reflect.DeepEqual(got, []string{"x", "z", "a"}) {
		t.Fatalf("Order = %q, %v", got, err)
	}
}
