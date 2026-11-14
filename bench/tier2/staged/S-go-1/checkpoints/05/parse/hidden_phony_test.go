package parse_test

import (
	"reflect"
	"testing"
)

func TestPhonyTargetsParsed(t *testing.T) {
	src := "ALL = all\n.PHONY: $(ALL) clean\nall: app\n\techo all\n.PHONY: clean test all\nclean:\n\trm -f app\ntest:\n"
	f := mustParse(t, src)
	if !reflect.DeepEqual(f.Phony, []string{"all", "clean", "test"}) {
		t.Fatalf("Phony = %q", f.Phony)
	}
	if f.Target(".PHONY") != nil || len(f.Targets) != 3 {
		t.Fatalf(".PHONY is not a target: %+v", f.Targets)
	}
	if got := f.Target("all").Commands; !reflect.DeepEqual(got, []string{"echo all"}) {
		t.Fatalf("all commands = %q", got)
	}
	if f.Phony == nil {
		t.Fatalf("Phony must be set")
	}
	if g := mustParse(t, "a:\n"); len(g.Phony) != 0 {
		t.Fatalf("Phony = %q", g.Phony)
	}
}

func TestPhonyErrors(t *testing.T) {
	cases := []struct {
		src  string
		line int
		msg  string
	}{
		{"a:\n.PHONY: a\n\techo\n", 3, "commands for .PHONY"},
		{".PHONY: a\nX = 1\n\n\techo\na:\n", 4, "commands for .PHONY"},
		{"a:\n.PHONY: a ghost\nb:\n.PHONY: other\n", 2, `phony "ghost" is not a target`},
		{".PHONY: x\n.PHONY: y x\nx:\n", 2, `phony "y" is not a target`},
	}
	for _, c := range cases {
		pe := parseErr(t, c.src)
		if pe.Line != c.line || pe.Msg != c.msg {
			t.Fatalf("%q: got %+v, want line %d %q", c.src, pe, c.line, c.msg)
		}
	}
}
