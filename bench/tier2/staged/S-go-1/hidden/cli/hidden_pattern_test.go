package cli_test

import (
	"testing"
)

func TestCLIPlanWithPatternRules(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", "OBJS = a.o b.o\nprog: $(OBJS)\n\tld -o $@ $^\n%.o: %.c\n\tcc -c $< # stem $*\n")
	mt := write(t, dir, "mtimes", "a.c 3\nb.c 3\na.o 4\nprog 5\n")
	code, out, errs := run("plan", bf, mt)
	if code != 0 || errs != "" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
	want := "b.o: missing\n  cc -c b.c # stem b\nprog: dependency b.o is being rebuilt\n  ld -o prog a.o b.o\n"
	if out != want {
		t.Fatalf("stdout = %q, want %q", out, want)
	}
	code, out, _ = run("order", bf)
	if code != 0 || out != "a.c\na.o\nb.c\nb.o\nprog\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
}
