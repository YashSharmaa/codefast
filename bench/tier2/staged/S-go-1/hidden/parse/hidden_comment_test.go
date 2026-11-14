package parse_test

import (
	"reflect"
	"testing"
)

const commented = "CC = gcc   # the compiler\nall: app   # the default goal\napp: main.c\n\t$(CC) -o app main.c\n"

func TestInlineCommentBugReport(t *testing.T) {
	f := mustParse(t, commented)
	if f.Vars["CC"] != "gcc" {
		t.Fatalf("CC = %q", f.Vars["CC"])
	}
	if got := f.Target("all").Deps; !reflect.DeepEqual(got, []string{"app"}) {
		t.Fatalf("all deps = %q", got)
	}
	if got := f.Target("app").Commands; !reflect.DeepEqual(got, []string{"gcc -o app main.c"}) {
		t.Fatalf("app commands = %q", got)
	}
}

func TestInlineCommentAfterAssignment(t *testing.T) {
	f := mustParse(t, "X = a#b\nY = # nothing\nZ=1 # one\nW = $(Z)$(Y)# tail\n")
	want := map[string]string{"X": "a", "Y": "", "Z": "1", "W": "1"}
	if !reflect.DeepEqual(f.Vars, want) {
		t.Fatalf("vars = %q", f.Vars)
	}
}

func TestInlineCommentAfterTargetLine(t *testing.T) {
	f := mustParse(t, "all: a b # main\nnone: # none\nx:y#z\n")
	if got := f.Target("all").Deps; !reflect.DeepEqual(got, []string{"a", "b"}) {
		t.Fatalf("all deps = %q", got)
	}
	if got := f.Target("none").Deps; len(got) != 0 {
		t.Fatalf("none deps = %q", got)
	}
	if got := f.Target("x").Deps; !reflect.DeepEqual(got, []string{"y"}) {
		t.Fatalf("x deps = %q", got)
	}
}

func TestInlineCommentHidesReferences(t *testing.T) {
	f := mustParse(t, "all: x # $(UNDEFINED)\nV = 1 # $(ALSO_UNDEFINED)\n")
	if got := f.Target("all").Deps; !reflect.DeepEqual(got, []string{"x"}) || f.Vars["V"] != "1" {
		t.Fatalf("all deps = %q, V = %q", got, f.Vars["V"])
	}
	pe := parseErr(t, "A = 1 # x\n\nall: $(B) # y\n")
	if pe.Line != 3 || pe.Msg != `undefined variable "B"` {
		t.Fatalf("got %+v", pe)
	}
}

func TestInlineCommentCanLeaveABadLine(t *testing.T) {
	cases := []struct {
		src  string
		line int
	}{
		{"all:\napp # : x\n", 2},
		{"# top\nX # = 1\n", 2},
		{"a:\n\n\nb # c\n", 4},
	}
	for _, c := range cases {
		pe := parseErr(t, c.src)
		if pe.Line != c.line || pe.Msg != "bad line" {
			t.Fatalf("%q: got %+v", c.src, pe)
		}
	}
}

func TestInlineCommentCommandsKeepHash(t *testing.T) {
	f := mustParse(t, "all:\n\techo # hi\n\t# a comment line\n   echo two#three \n")
	if got := f.Target("all").Commands; !reflect.DeepEqual(got, []string{"echo # hi", "echo two#three"}) {
		t.Fatalf("commands = %q", got)
	}
}

func TestInlineCommentOnPatternRuleLine(t *testing.T) {
	f := mustParse(t, "%.o: %.c # objects\n\tcc -c $< # compile\n")
	if len(f.Rules) != 1 || !reflect.DeepEqual(f.Rules[0].Deps, []string{"%.c"}) {
		t.Fatalf("rules = %+v", f.Rules)
	}
	if got := f.Resolve("a.o"); got == nil || !reflect.DeepEqual(got.Commands, []string{"cc -c a.c # compile"}) {
		t.Fatalf("a.o = %+v", got)
	}
}
