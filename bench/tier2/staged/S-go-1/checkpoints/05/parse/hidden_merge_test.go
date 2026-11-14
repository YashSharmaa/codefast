package parse_test

import (
	"reflect"
	"testing"
)

func TestMergedDependenciesKeepFirstPosition(t *testing.T) {
	f := mustParse(t, "a: x y\nb:\na: y z x w\n")
	if len(f.Targets) != 2 || f.Targets[0].Name != "a" || f.Targets[1].Name != "b" {
		t.Fatalf("targets = %+v", f.Targets)
	}
	a := f.Targets[0]
	if a.Line != 1 || !reflect.DeepEqual(a.Deps, []string{"x", "y", "z", "w"}) {
		t.Fatalf("a = %+v", a)
	}
}

func TestMergedCommandsFollowOneLine(t *testing.T) {
	f := mustParse(t, "a: x\nb:\n\tbuild b\na: y\n\tbuild a\n\n# note\n\tlink a\n")
	a := f.Target("a")
	if !reflect.DeepEqual(a.Commands, []string{"build a", "link a"}) || !reflect.DeepEqual(a.Deps, []string{"x", "y"}) || a.Line != 1 {
		t.Fatalf("a = %+v", a)
	}
	if got := f.Target("b").Commands; !reflect.DeepEqual(got, []string{"build b"}) {
		t.Fatalf("b commands = %q", got)
	}
}

func TestMergedLinesWithoutCommandsAreFine(t *testing.T) {
	f := mustParse(t, "a:\n\tone\nb:\n\ttwo\na: c\nb: d\na: e\n")
	if got := f.Target("a"); !reflect.DeepEqual(got.Commands, []string{"one"}) || !reflect.DeepEqual(got.Deps, []string{"c", "e"}) {
		t.Fatalf("a = %+v", got)
	}
	if got := f.Target("b"); !reflect.DeepEqual(got.Commands, []string{"two"}) || !reflect.DeepEqual(got.Deps, []string{"d"}) {
		t.Fatalf("b = %+v", got)
	}
}

func TestDuplicateCommandsError(t *testing.T) {
	pe := parseErr(t, "app: main.o\n\tld $^\n\n\napp: util.o main.o\n\techo\n")
	if pe.Line != 6 || pe.Msg != `duplicate commands for "app"` || pe.Error() != `line 6: duplicate commands for "app"` {
		t.Fatalf("got %+v", pe)
	}
	pe = parseErr(t, "a:\nb:\n\tx\na:\n\tone\nb: q\n\n\ttwo\n")
	if pe.Line != 8 || pe.Msg != `duplicate commands for "b"` {
		t.Fatalf("got %+v", pe)
	}
}

func TestMergedAutomaticVariables(t *testing.T) {
	f := mustParse(t, "app: main.o\n\tld $^ first=$<\n\n\n\napp: util.o main.o\n")
	app := f.Target("app")
	if app.Line != 1 || !reflect.DeepEqual(app.Deps, []string{"main.o", "util.o"}) {
		t.Fatalf("app = %+v", app)
	}
	if !reflect.DeepEqual(app.Commands, []string{"ld main.o util.o first=main.o"}) {
		t.Fatalf("commands = %q", app.Commands)
	}
}

func TestMergedLinesExpandWithCurrentValues(t *testing.T) {
	f := mustParse(t, "X = 1\na: $(X)\nX = 2\na: $(X) $(X) 1\n")
	if got := f.Target("a").Deps; !reflect.DeepEqual(got, []string{"1", "2"}) {
		t.Fatalf("deps = %q", got)
	}
	pe := parseErr(t, "N = a\n$(N):\n\tok\n$(N): $(MISSING)\n")
	if pe.Line != 4 || pe.Msg != `undefined variable "MISSING"` {
		t.Fatalf("got %+v", pe)
	}
}

func TestMergedTargetWinsOverRule(t *testing.T) {
	f := mustParse(t, "x.o: x.c\n%.o: %.c\n\tcc $<\nx.o: x.h\n")
	r := f.Resolve("x.o")
	if r != f.Target("x.o") || !reflect.DeepEqual(r.Deps, []string{"x.c", "x.h"}) || len(r.Commands) != 0 {
		t.Fatalf("x.o = %+v", r)
	}
	if len(f.Rules) != 1 || len(f.Rules[0].Commands) != 1 {
		t.Fatalf("rules = %+v", f.Rules)
	}
}

func TestAppendAssignment(t *testing.T) {
	f := mustParse(t, "SRC = a.c\nSRC += b.c\nSRC +=\nE =\nE += x\nNEW\t+=  $(SRC)  z\nall: $(SRC)\n\techo $(NEW)\nSRC += c.c\n")
	want := map[string]string{"SRC": "a.c b.c c.c", "E": "x", "NEW": "a.c b.c  z"}
	if !reflect.DeepEqual(f.Vars, want) {
		t.Fatalf("vars = %q", f.Vars)
	}
	all := f.Target("all")
	if !reflect.DeepEqual(all.Deps, []string{"a.c", "b.c"}) || !reflect.DeepEqual(all.Commands, []string{"echo a.c b.c  z"}) {
		t.Fatalf("all = %+v", all)
	}
	g := mustParse(t, "U +=\nall: [$(U)]\n")
	if v, ok := g.Vars["U"]; !ok || v != "" || !reflect.DeepEqual(g.Target("all").Deps, []string{"[]"}) {
		t.Fatalf("vars = %q, deps = %q", g.Vars, g.Target("all").Deps)
	}
}

func TestAppendLineIsNoLongerATarget(t *testing.T) {
	f := mustParse(t, "X+=a:b\nY = 1\nY+=$(X)\n")
	if len(f.Targets) != 0 || f.Vars["X"] != "a:b" || f.Vars["Y"] != "1 a:b" {
		t.Fatalf("targets = %+v, vars = %q", f.Targets, f.Vars)
	}
	pe := parseErr(t, "X += $(NOPE)\n")
	if pe.Line != 1 || pe.Msg != `undefined variable "NOPE"` {
		t.Fatalf("got %+v", pe)
	}
}
