package cli_test

import "testing"

const commentedfile = "CC = gcc   # the compiler\nall: app   # the default goal\napp: main.c\n\t$(CC) -o app main.c\n"

func TestCLIInlineCommentsInBuildFile(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", commentedfile)
	code, out, errs := run("order", bf)
	if code != 0 || errs != "" || out != "main.c\napp\nall\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
	mt := write(t, dir, "mtimes", "main.c 1\n")
	code, out, errs = run("plan", bf, mt)
	if code != 0 || out != "app: missing\n  gcc -o app main.c\nall: missing\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}

func TestCLIInlineCommentsInMtimes(t *testing.T) {
	dir := t.TempDir()
	bf := write(t, dir, "Buildfile", commentedfile)
	mt := write(t, dir, "mtimes", "main.c 10 # edited\n   # only a comment\napp 20#built\nall 30 #\n")
	code, out, errs := run("plan", bf, mt)
	if code != 0 || errs != "" || out != "nothing to do\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
	bad := write(t, dir, "bad", "app 20\n\nmain.c # 10\n")
	code, out, errs = run("plan", bf, bad)
	if code != 2 || out != "" || errs != "error: mtimes line 3: bad line\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}
