package parse_test

import (
	"errors"
	"reflect"
	"strings"
	"testing"

	"example.com/buildplan/parse"
)

func mustParse(t *testing.T, src string) *parse.File {
	t.Helper()
	f, err := parse.Parse(src)
	if err != nil {
		t.Fatalf("Parse: unexpected error %v", err)
	}
	return f
}

func parseErr(t *testing.T, src string) *parse.ParseError {
	t.Helper()
	_, err := parse.Parse(src)
	if err == nil {
		t.Fatalf("Parse: expected an error")
	}
	var pe *parse.ParseError
	if !errors.As(err, &pe) {
		t.Fatalf("error %v (%T) is not *parse.ParseError", err, err)
	}
	return pe
}

func TestParseTargetsDepsCommandsAndLines(t *testing.T) {
	src := "app: main.o util.o\n\tcc -o app main.o util.o\n\techo done\n\nmain.o: main.c\n  cc -c main.c\nutil.o:\n"
	f := mustParse(t, src)
	if len(f.Targets) != 3 {
		t.Fatalf("got %d targets", len(f.Targets))
	}
	app := f.Targets[0]
	if app.Name != "app" || app.Line != 1 {
		t.Fatalf("app = %+v", app)
	}
	if !reflect.DeepEqual(app.Deps, []string{"main.o", "util.o"}) {
		t.Fatalf("app deps = %q", app.Deps)
	}
	if !reflect.DeepEqual(app.Commands, []string{"cc -o app main.o util.o", "echo done"}) {
		t.Fatalf("app commands = %q", app.Commands)
	}
	m := f.Target("main.o")
	if m == nil || m.Line != 5 || !reflect.DeepEqual(m.Deps, []string{"main.c"}) || !reflect.DeepEqual(m.Commands, []string{"cc -c main.c"}) {
		t.Fatalf("main.o = %+v", m)
	}
	u := f.Target("util.o")
	if u == nil || u.Line != 7 || len(u.Deps) != 0 || len(u.Commands) != 0 {
		t.Fatalf("util.o = %+v", u)
	}
	if f.Target("main.c") != nil {
		t.Fatalf("main.c is not a declared target")
	}
}

func TestParseVariablesExpandInNamesDepsAndCommands(t *testing.T) {
	src := "CC = gcc\nOUT=prog\nOBJS = a.o  b.o\nFLAGS = -O2 $(CC)\n$(OUT): $(OBJS) lib.a\n\t$(CC) $(FLAGS) -o $(OUT)\nCC = clang\nlib.a:\n\t$(CC) ar\n"
	f := mustParse(t, src)
	p := f.Target("prog")
	if p == nil {
		t.Fatalf("target name was not expanded: %+v", f.Targets)
	}
	if !reflect.DeepEqual(p.Deps, []string{"a.o", "b.o", "lib.a"}) {
		t.Fatalf("deps = %q", p.Deps)
	}
	if !reflect.DeepEqual(p.Commands, []string{"gcc -O2 gcc -o prog"}) {
		t.Fatalf("commands = %q", p.Commands)
	}
	if got := f.Target("lib.a").Commands; !reflect.DeepEqual(got, []string{"clang ar"}) {
		t.Fatalf("lib.a commands = %q (reassignment applies to later lines)", got)
	}
	want := map[string]string{"CC": "clang", "OUT": "prog", "OBJS": "a.o  b.o", "FLAGS": "-O2 gcc"}
	if !reflect.DeepEqual(f.Vars, want) {
		t.Fatalf("vars = %q", f.Vars)
	}
}

func TestParseCommentsBlankLinesAndCRLF(t *testing.T) {
	src := "# header comment\r\nall: x\r\n\techo one\r\n\r\n   # indented comment\r\n\r\n\techo two\r\n  \t \r\nx:\r\n"
	f := mustParse(t, src)
	if len(f.Targets) != 2 {
		t.Fatalf("got %d targets", len(f.Targets))
	}
	all := f.Targets[0]
	if all.Line != 2 || !reflect.DeepEqual(all.Deps, []string{"x"}) {
		t.Fatalf("all = %+v", all)
	}
	if !reflect.DeepEqual(all.Commands, []string{"echo one", "echo two"}) {
		t.Fatalf("commands = %q", all.Commands)
	}
	if f.Targets[1].Name != "x" || f.Targets[1].Line != 9 {
		t.Fatalf("x = %+v", f.Targets[1])
	}
}

func TestParseRepeatedTargetLines(t *testing.T) {
	f := mustParse(t, "a: b\n\techo\n\nb:\na: c\n")
	if len(f.Targets) != 2 {
		t.Fatalf("got %d targets", len(f.Targets))
	}
	a := f.Targets[0]
	if a.Name != "a" || a.Line != 1 || !reflect.DeepEqual(a.Deps, []string{"b", "c"}) || !reflect.DeepEqual(a.Commands, []string{"echo"}) {
		t.Fatalf("a = %+v", a)
	}
	pe := parseErr(t, "a:\n\tok\na: $(UNDEFINED)\n")
	if pe.Line != 3 || pe.Msg != `undefined variable "UNDEFINED"` {
		t.Fatalf("the dependency text of a repeated target is expanded: %+v", pe)
	}
	if pe.Error() != `line 3: undefined variable "UNDEFINED"` {
		t.Fatalf("Error() = %q", pe.Error())
	}
}

func TestParseCommandBeforeAnyTarget(t *testing.T) {
	pe := parseErr(t, "# c\nX = 1\n\n\techo hi\nall:\n")
	if pe.Line != 4 || pe.Msg != "command before any target" {
		t.Fatalf("got %+v", pe)
	}
}

func TestParseUndefinedVariable(t *testing.T) {
	pe := parseErr(t, "all: $(SRC)\nSRC = a.c\n")
	if pe.Line != 1 || pe.Msg != `undefined variable "SRC"` {
		t.Fatalf("got %+v", pe)
	}
	pe = parseErr(t, "A = 1\nall:\n\techo $(A) $(B)\n")
	if pe.Line != 3 || pe.Msg != `undefined variable "B"` {
		t.Fatalf("got %+v", pe)
	}
	pe = parseErr(t, "A = $(A)\n")
	if pe.Line != 1 || pe.Msg != `undefined variable "A"` {
		t.Fatalf("got %+v", pe)
	}
}

func TestParseBadLines(t *testing.T) {
	cases := []struct {
		src  string
		line int
	}{
		{"all:\njust some words\n", 2},
		{"x:\n: dep\n", 2},
		{"TWO = a b\n$(TWO): c\n", 2},
		{"= value\n", 1},
		{"E =\n$(E) : x\n", 2},
	}
	for _, c := range cases {
		pe := parseErr(t, c.src)
		if pe.Line != c.line || pe.Msg != "bad line" {
			t.Fatalf("%q: got %+v", c.src, pe)
		}
	}
}

func TestParseDollarKeptLiterallyAndNoRescan(t *testing.T) {
	src := "D = $\nE = (X)\nX = no\nR = $(D)(X)\nall:\n\techo $$ $x $(1) $(X $(D)$(E) cost$ $(R)\n"
	f := mustParse(t, src)
	if f.Vars["R"] != "$(X)" {
		t.Fatalf("R = %q, inserted text must not be expanded again", f.Vars["R"])
	}
	got := f.Target("all").Commands
	want := []string{"echo $$ $x $(1) $(X $(X) cost$ $(X)"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("commands = %q, want %q", got, want)
	}
}

func TestParseAssignmentRulesAndRepeatedDeps(t *testing.T) {
	src := "URL = http://example.com/a:b\nNAME=x\nall: b a b c a\n\tget $(URL)\n"
	f := mustParse(t, src)
	if f.Vars["URL"] != "http://example.com/a:b" || f.Vars["NAME"] != "x" {
		t.Fatalf("vars = %q", f.Vars)
	}
	if len(f.Targets) != 1 {
		t.Fatalf("assignment with ':' in value must not be a target: %+v", f.Targets)
	}
	if !reflect.DeepEqual(f.Targets[0].Deps, []string{"b", "a", "c"}) {
		t.Fatalf("deps = %q", f.Targets[0].Deps)
	}
	if !strings.HasSuffix(f.Targets[0].Commands[0], "a:b") {
		t.Fatalf("command = %q", f.Targets[0].Commands[0])
	}
}
