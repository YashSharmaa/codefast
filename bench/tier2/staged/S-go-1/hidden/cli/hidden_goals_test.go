package cli_test

import "testing"

const rulesfile = "CC = cc\nprog: a.o\n\t$(CC) -o $@ $^\n%.o: %.c\n\t$(CC) -c $<\n"

func TestCLIOrderGoalWithRule(t *testing.T) {
	bf := write(t, t.TempDir(), "Buildfile", rulesfile)
	code, out, errs := run("order", bf, "z.o", "prog")
	if code != 0 || errs != "" || out != "z.c\nz.o\na.c\na.o\nprog\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}

func TestCLIPlanAndWavesGoalWithRule(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", rulesfile)
	mt := write(t, dir, "mtimes", "a.c 1\nz.c 1\nq.c 1\n")
	code, out, _ := run("plan", bf, mt, "z.o")
	if code != 0 || out != "z.o: missing\n  cc -c z.c\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
	code, out, _ = run("waves", bf, mt, "-j", "2", "z.o", "prog", "q.o")
	if code != 0 || out != "wave 1: z.o a.o\nwave 2: prog q.o\ncritical: a.o prog\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
	code, out, errs := run("plan", bf, mt, "w.o")
	if code != 1 || out != "" || errs != "error: missing \"w.c\" needed by \"w.o\"\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}

func TestCLICyclesIgnoresRulesForNonDependencies(t *testing.T) {
	bf := write(t, t.TempDir(), "Buildfile", "app: lib.a\n%.a: %.o\n%.o: app\n")
	code, out, _ := run("cycles", bf)
	if code != 0 || out != "no cycles\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
}
