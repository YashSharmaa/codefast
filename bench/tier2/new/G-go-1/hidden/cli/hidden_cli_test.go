package cli_test

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"example.com/buildplan/cli"
)

const buildfile = `# demo project
CC = cc
app: main.o util.o
	$(CC) -o app main.o util.o
main.o: main.c
	$(CC) -c main.c
util.o: util.c
	$(CC) -c util.c
clean:
	rm -f app *.o
`

func write(t *testing.T, dir, name, content string) string {
	t.Helper()
	p := filepath.Join(dir, name)
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

func run(args ...string) (int, string, string) {
	var out, errb bytes.Buffer
	code := cli.Run(args, &out, &errb)
	return code, out.String(), errb.String()
}

func TestCLIOrderGoals(t *testing.T) {
	bf := write(t, t.TempDir(), "Buildfile", buildfile)
	code, out, errs := run("order", bf, "util.o", "app")
	if code != 0 || errs != "" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
	if out != "util.c\nutil.o\nmain.c\nmain.o\napp\n" {
		t.Fatalf("stdout = %q", out)
	}
}

func TestCLIOrderDefaultGoal(t *testing.T) {
	bf := write(t, t.TempDir(), "Buildfile", "X = y\nfirst: $(X)\nsecond: first\n")
	code, out, _ := run("order", bf)
	if code != 0 || out != "y\nfirst\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
}

func TestCLIOrderCycleExitsOne(t *testing.T) {
	bf := write(t, t.TempDir(), "Buildfile", "all: a\na: b\nb: a\n")
	code, out, errs := run("order", bf)
	if code != 1 || out != "" || errs != "error: cycle: a -> b -> a\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}

func TestCLIPlanOutput(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", buildfile)
	mt := write(t, dir, "mtimes", "# name time\nmain.c 10\nutil.c 30\n\n  main.o 20\nutil.o 20\napp 25\nutil.c 40\n")
	code, out, errs := run("plan", bf, mt)
	if code != 0 || errs != "" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
	want := "util.o: dependency util.c is newer\n  cc -c util.c\napp: dependency util.o is being rebuilt\n  cc -o app main.o util.o\n"
	if out != want {
		t.Fatalf("stdout = %q, want %q", out, want)
	}
}

func TestCLIPlanGoalsAndNothingToDo(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", buildfile)
	mt := write(t, dir, "mtimes", "main.c 1\nutil.c 1\nmain.o 2\n")
	code, out, _ := run("plan", bf, mt, "main.o")
	if code != 0 || out != "nothing to do\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
	code, out, _ = run("plan", bf, mt, "clean", "util.o")
	if code != 0 || out != "clean: missing\n  rm -f app *.o\nutil.o: missing\n  cc -c util.c\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
}

func TestCLIPlanMissingExitsOne(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", buildfile)
	mt := write(t, dir, "mtimes", "main.c 1\n")
	code, out, errs := run("plan", bf, mt)
	if code != 1 || out != "" || errs != "error: missing \"util.c\" needed by \"util.o\"\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}

func TestCLIParseAndMtimesErrorsExitTwo(t *testing.T) {
	dir := t.TempDir()
	bad := write(t, dir, "Bad", "all: x\n\techo\nwhat is this\n")
	code, out, errs := run("order", bad)
	if code != 2 || out != "" || errs != "error: line 3: bad line\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
	bf := write(t, dir, "Buildfile", buildfile)
	mt := write(t, dir, "mtimes", "main.c 1\n\nutil.c soon\n")
	code, out, errs = run("plan", bf, mt)
	if code != 2 || out != "" || errs != "error: mtimes line 3: bad line\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
	mt2 := write(t, dir, "mtimes2", "main.c 1 2\n")
	if code, _, errs = run("plan", bf, mt2); code != 2 || errs != "error: mtimes line 1: bad line\n" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
}

func TestCLINoTargetsAndUnreadableFile(t *testing.T) {
	dir := t.TempDir()
	empty := write(t, dir, "Empty", "# only a comment\nA = 1\n")
	code, _, errs := run("order", empty)
	if code != 2 || errs != "error: no targets\n" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
	code, out, errs := run("cycles", filepath.Join(dir, "does-not-exist"))
	if code != 2 || out != "" || !strings.HasPrefix(errs, "error: ") {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}

func TestCLICycles(t *testing.T) {
	dir := t.TempDir()
	cyc := write(t, dir, "Cyc", "all: c\nc: b\nb: c\nself: self\nok: leaf\n")
	code, out, _ := run("cycles", cyc)
	if code != 1 || out != "b c\nself\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
	clean := write(t, dir, "Clean", buildfile)
	code, out, _ = run("cycles", clean)
	if code != 0 || out != "no cycles\n" {
		t.Fatalf("code=%d stdout=%q", code, out)
	}
}

func TestCLIUsageErrors(t *testing.T) {
	bf := write(t, t.TempDir(), "Buildfile", buildfile)
	for _, args := range [][]string{{}, {"build", bf}, {"order"}, {"plan", bf}, {"cycles"}, {"cycles", bf, "extra"}} {
		code, out, errs := run(args...)
		if code != 2 || out != "" || !strings.HasPrefix(errs, "usage:") {
			t.Fatalf("args %q: code=%d stdout=%q stderr=%q", args, code, out, errs)
		}
	}
}
