package parse_test

import (
	"reflect"
	"testing"
)

func TestPatternRuleIsParsedAsRule(t *testing.T) {
	src := "CC = cc\nH = defs.h\napp: main.o\n\tld $@\n%.o: %.c $(H) %.c\n\t$(CC) -c $< -o $@\n\n\techo $*\nmain.o: special.c\n"
	f := mustParse(t, src)
	if len(f.Targets) != 2 || f.Targets[0].Name != "app" || f.Targets[1].Name != "main.o" {
		t.Fatalf("targets = %+v", f.Targets)
	}
	if f.Target("%.o") != nil {
		t.Fatalf("a pattern rule is not a declared target")
	}
	if len(f.Rules) != 1 {
		t.Fatalf("got %d rules", len(f.Rules))
	}
	r := f.Rules[0]
	if r.Pattern != "%.o" || r.Line != 5 {
		t.Fatalf("rule = %+v", r)
	}
	if !reflect.DeepEqual(r.Deps, []string{"%.c", "defs.h"}) {
		t.Fatalf("rule deps = %q", r.Deps)
	}
	if !reflect.DeepEqual(r.Commands, []string{"cc -c $< -o $@", "echo $*"}) {
		t.Fatalf("rule commands = %q (automatic variables stay until the rule is used)", r.Commands)
	}
	if got := f.Target("app").Commands; !reflect.DeepEqual(got, []string{"ld app"}) {
		t.Fatalf("app commands = %q", got)
	}
}

func TestPatternRuleErrors(t *testing.T) {
	pe := parseErr(t, "%.o: %.c\n\tcc\n%.o: %.s\n")
	if pe.Line != 3 || pe.Msg != `duplicate rule "%.o"` {
		t.Fatalf("got %+v", pe)
	}
	cases := []struct {
		src  string
		line int
	}{
		{"a: b\n%.%: x\n", 2},
		{"a:\nP = %\nlib$(P)$(P).a: x\n", 3},
	}
	for _, c := range cases {
		pe = parseErr(t, c.src)
		if pe.Msg != "bad line" || pe.Line != c.line {
			t.Fatalf("%q: got %+v", c.src, pe)
		}
	}
	f := mustParse(t, "lib%.a: %.o\n%.a: x\n")
	if len(f.Rules) != 2 || f.Rules[0].Pattern != "lib%.a" || f.Rules[1].Pattern != "%.a" {
		t.Fatalf("rules = %+v", f.Rules)
	}
}

func TestAutomaticVariablesInDeclaredTargets(t *testing.T) {
	src := "T = <$@>\napp: main.o util.o main.o\n\tld -o $@ $^\n\techo $< [$*] $(T)\n\techo $$ $x $(1) $\nclean:\n\techo [$<][$^][$@]\n"
	f := mustParse(t, src)
	want := []string{"ld -o app main.o util.o", "echo main.o [] <app>", "echo $$ $x $(1) $"}
	if got := f.Target("app").Commands; !reflect.DeepEqual(got, want) {
		t.Fatalf("app commands = %q, want %q", got, want)
	}
	if got := f.Target("clean").Commands; !reflect.DeepEqual(got, []string{"echo [][][clean]"}) {
		t.Fatalf("clean commands = %q", got)
	}
}

func TestResolveDeclaredAndInstantiated(t *testing.T) {
	src := "main.o: main.c\n\tcc main\nlib%.o: src/lib%.c\n\tcc lib $*\n%.o: %.c gen/%_%.h %.c\n\tcc -c $< -o $@ ($^) $*\n"
	f := mustParse(t, src)
	if f.Resolve("main.o") != f.Target("main.o") {
		t.Fatalf("Resolve of a declared target must return that target")
	}
	u := f.Resolve("util.o")
	if u == nil || u.Name != "util.o" || u.Line != 5 {
		t.Fatalf("util.o = %+v", u)
	}
	if !reflect.DeepEqual(u.Deps, []string{"util.c", "gen/util_util.h"}) {
		t.Fatalf("util.o deps = %q", u.Deps)
	}
	if !reflect.DeepEqual(u.Commands, []string{"cc -c util.c -o util.o (util.c gen/util_util.h) util"}) {
		t.Fatalf("util.o commands = %q", u.Commands)
	}
	l := f.Resolve("libz.o")
	if l == nil || l.Line != 3 || !reflect.DeepEqual(l.Deps, []string{"src/libz.c"}) || !reflect.DeepEqual(l.Commands, []string{"cc lib z"}) {
		t.Fatalf("first matching rule must win: %+v", l)
	}
	if f.Target("util.o") != nil {
		t.Fatalf("Resolve must not declare the target")
	}
	for _, name := range []string{".o", "util.c", "o", ""} {
		if r := f.Resolve(name); r != nil {
			t.Fatalf("Resolve(%q) = %+v, want nil (the stem must not be empty)", name, r)
		}
	}
}
