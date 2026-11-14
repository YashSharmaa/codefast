package graph_test

import (
	"reflect"
	"testing"
)

func TestGraphInstantiatesRulesWithoutChaining(t *testing.T) {
	g := fromSrc(t, "app: main.o x.o\nmain.o: special.c\n%.o: %.c defs.h\n%.c: %.y\n")
	want := []string{"app", "main.o", "x.o", "special.c", "x.c", "defs.h", "special.y"}
	if got := g.Nodes(); !reflect.DeepEqual(got, want) {
		t.Fatalf("Nodes = %q, want %q", got, want)
	}
	if got := g.Deps("x.o"); !reflect.DeepEqual(got, []string{"x.c", "defs.h"}) {
		t.Fatalf("Deps(x.o) = %q", got)
	}
	if got := g.Deps("x.c"); len(got) != 0 {
		t.Fatalf("x.c comes from a rule target and must not be matched again: %q", got)
	}
	if got := g.Deps("main.o"); !reflect.DeepEqual(got, []string{"special.c"}) {
		t.Fatalf("a declared target never takes a rule: %q", got)
	}
	got, err := g.Order([]string{"app"})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, []string{"special.y", "special.c", "main.o", "x.c", "defs.h", "x.o", "app"}) {
		t.Fatalf("Order = %q", got)
	}
}

func TestGraphRuleEdgesCanCloseCycles(t *testing.T) {
	g := fromSrc(t, "app: x.o\n%.o: app\n")
	if got := g.Cycles(); !reflect.DeepEqual(got, [][]string{{"app", "x.o"}}) {
		t.Fatalf("Cycles = %q", got)
	}
	_, err := g.Order([]string{"app"})
	if ce := cycleErr(t, err); !reflect.DeepEqual(ce.Path, []string{"app", "x.o", "app"}) {
		t.Fatalf("Path = %q", ce.Path)
	}
}
