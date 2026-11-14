package cli_test

import "testing"

func TestCLIMergedTargets(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", "all: b\nb: c\nall: a b\n")
	code, out, errs := run("order", bf)
	if code != 0 || errs != "" || out != "c\nb\na\nall\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
	bad := write(t, dir, "Bad", "a:\n\tone\na: b\n\ttwo\n")
	code, out, errs = run("order", bad)
	if code != 2 || out != "" || errs != "error: line 4: duplicate commands for \"a\"\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}
