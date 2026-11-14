package cli_test

import (
	"strings"
	"testing"
)

const wavesfile = `all: app docs
app: main.o util.o
	ld -o app main.o util.o
main.o: main.c
	cc -c main.c
util.o: util.c
	cc -c util.c
docs: guide.md
	render guide.md
`

func TestCLIWavesOutput(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", wavesfile)
	mt := write(t, dir, "mtimes", "main.c 1\nutil.c 1\nguide.md 1\n")
	code, out, errs := run("waves", bf, mt)
	if code != 0 || errs != "" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
	if want := "wave 1: main.o util.o docs\nwave 2: app\nwave 3: all\ncritical: main.o app all\n"; out != want {
		t.Fatalf("stdout = %q, want %q", out, want)
	}
}

func TestCLIWavesJobsAndGoals(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", wavesfile)
	mt := write(t, dir, "mtimes", "main.c 1\nutil.c 1\nguide.md 1\n")
	code, out, _ := run("waves", bf, mt, "-j", "1", "app")
	if code != 0 || out != "wave 1: main.o\nwave 2: util.o\nwave 3: app\ncritical: main.o app\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
	code, out, _ = run("waves", bf, mt, "-j", "2")
	if code != 0 || out != "wave 1: main.o util.o\nwave 2: app docs\nwave 3: all\ncritical: main.o app all\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
	code, out, errs := run("waves", bf, mt, "docs", "-j", "2")
	if code != 1 || out != "" || errs != "error: missing \"-j\"\n" {
		t.Fatalf("-j after a goal is a goal: code=%d stdout=%q stderr=%q", code, out, errs)
	}
}

func TestCLIWavesNothingToDoAndErrors(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", wavesfile)
	mt := write(t, dir, "mtimes", "main.c 1\nutil.c 1\nguide.md 1\nmain.o 2\nutil.o 2\napp 3\ndocs 2\nall 4\n")
	code, out, _ := run("waves", bf, mt)
	if code != 0 || out != "nothing to do\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
	missing := write(t, dir, "mtimes2", "main.c 1\n")
	code, out, errs := run("waves", bf, missing, "app")
	if code != 1 || out != "" || errs != "error: missing \"util.c\" needed by \"util.o\"\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
	cyc := write(t, dir, "Cyc", "a: b\nb: a\n")
	code, _, errs = run("waves", cyc, mt)
	if code != 1 || errs != "error: cycle: a -> b -> a\n" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
	bad := write(t, dir, "bad-mtimes", "main.c one\n")
	code, _, errs = run("waves", bf, bad)
	if code != 2 || errs != "error: mtimes line 1: bad line\n" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
}

func TestCLIWavesUsageErrors(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", wavesfile)
	mt := write(t, dir, "mtimes", "main.c 1\n")
	for _, args := range [][]string{{"waves"}, {"waves", bf}, {"waves", bf, mt, "-j"}, {"waves", bf, mt, "-j", "0"},
		{"waves", bf, mt, "-j", "x"}, {"waves", bf, mt, "-j", "+2"}, {"waves", bf, mt, "-j", "-1"}, {"waves", bf, mt, "-j", ""}} {
		code, out, errs := run(args...)
		if code != 2 || out != "" || !strings.HasPrefix(errs, "usage:") {
			t.Fatalf("args %q: code=%d stdout=%q stderr=%q", args[1:], code, out, errs)
		}
	}
}
