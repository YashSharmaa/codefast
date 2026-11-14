package cli_test

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"example.com/journal/cli"
	"example.com/journal/journal"
)

func run(args ...string) (int, string, string) {
	var out, errb bytes.Buffer
	code := cli.Run(args, &out, &errb)
	return code, out.String(), errb.String()
}

func must(t *testing.T, want string, args ...string) {
	t.Helper()
	code, out, errs := run(args...)
	if code != 0 || out != want || errs != "" {
		t.Fatalf("%q: code=%d stdout=%q stderr=%q, want stdout %q", args, code, out, errs, want)
	}
}

func TestCLIAppendAndDump(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "j")
	must(t, "1\n", "append", dir, "add", "apple 5")
	must(t, "2\n", "append", dir, "note", `say "hi"`+"\n")
	must(t, "3\n", "append", dir, "add", "")
	must(t, "1 add \"apple 5\"\n2 note \"say \\\"hi\\\"\\n\"\n3 add \"\"\n", "dump", dir)
	must(t, "2 note \"say \\\"hi\\\"\\n\"\n3 add \"\"\n", "dump", dir, "2")
	must(t, "", "dump", dir, "4")
	if _, err := os.Stat(filepath.Join(dir, "00000000000000000001.log")); err != nil {
		t.Fatalf("segment file missing: %v", err)
	}
}

func TestCLIInventoryCommands(t *testing.T) {
	dir := t.TempDir()
	must(t, "1\n", "add", dir, "widget", "10")
	must(t, "2\n", "add", dir, "bolt", "3")
	must(t, "3\n", "remove", dir, "widget", "4")
	must(t, "4\n", "remove", dir, "bolt", "3")
	must(t, "widget 6\n", "stock", dir)
	must(t, "1 add \"widget 10\"\n2 add \"bolt 3\"\n3 remove \"widget 4\"\n4 remove \"bolt 3\"\n", "dump", dir)
	empty := t.TempDir()
	must(t, "", "stock", empty)
}

func TestCLIRemoveInsufficient(t *testing.T) {
	dir := t.TempDir()
	must(t, "1\n", "add", dir, "gear", "2")
	code, out, errs := run("remove", dir, "gear", "3")
	if code != 1 || out != "" || errs != "error: inventory: insufficient stock\n" {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
	code, _, errs = run("add", dir, "bad/sku", "1")
	if code != 1 || errs != "error: inventory: malformed\n" {
		t.Fatalf("code=%d stderr=%q", code, errs)
	}
	must(t, "1 add \"gear 2\"\n", "dump", dir)
}

func TestCLISnapshot(t *testing.T) {
	dir := t.TempDir()
	code, _, errs := run("snapshot", dir)
	if code != 1 || !strings.HasPrefix(errs, "error: ") {
		t.Fatalf("snapshot of empty journal: code=%d stderr=%q", code, errs)
	}
	must(t, "1\n", "add", dir, "cog", "5")
	must(t, "2\n", "add", dir, "axle", "1")
	must(t, "snapshot 2\n", "snapshot", dir)
	data, err := os.ReadFile(filepath.Join(dir, "snapshot-00000000000000000002.snap"))
	if err != nil || string(data[4:]) != "axle 1\ncog 5\n" {
		t.Fatalf("snapshot file = %q, %v", data, err)
	}
	must(t, "3\n", "remove", dir, "cog", "1")
	must(t, "axle 1\ncog 4\n", "stock", dir)
}

func TestCLICompact(t *testing.T) {
	dir := t.TempDir()
	j, err := journal.Open(dir, journal.Options{SegmentSize: 30})
	if err != nil {
		t.Fatal(err)
	}
	for i := 1; i <= 4; i++ {
		j.Append("add", []byte(fmt.Sprintf("p%d 1", i)))
	}
	j.Close()
	must(t, "removed 0\n", "compact", dir, "1")
	must(t, "removed 2\n", "compact", dir, "3")
	must(t, "3 add \"p3 1\"\n4 add \"p4 1\"\n", "dump", dir, "3")
	code, out, errs := run("dump", dir)
	if code != 1 || out != "" || !strings.HasPrefix(errs, "error: ") {
		t.Fatalf("dump of compacted range: code=%d stdout=%q stderr=%q", code, out, errs)
	}
	code, _, _ = run("stock", dir)
	if code != 1 {
		t.Fatalf("stock without snapshot after compaction: code=%d", code)
	}
}

func TestCLICorruptJournal(t *testing.T) {
	dir := t.TempDir()
	must(t, "1\n", "append", dir, "add", "a 1")
	must(t, "2\n", "append", dir, "add", "b 1")
	p := filepath.Join(dir, "00000000000000000001.log")
	data, _ := os.ReadFile(p)
	data[10] ^= 0xff
	os.WriteFile(p, data, 0o644)
	code, out, errs := run("dump", dir)
	if code != 1 || out != "" || !strings.HasPrefix(errs, "error: ") {
		t.Fatalf("code=%d stdout=%q stderr=%q", code, out, errs)
	}
}

func TestCLITornTailRecovered(t *testing.T) {
	dir := t.TempDir()
	must(t, "1\n", "add", dir, "a", "1")
	must(t, "2\n", "add", dir, "b", "2")
	p := filepath.Join(dir, "00000000000000000001.log")
	st, _ := os.Stat(p)
	os.Truncate(p, st.Size()-3)
	must(t, "a 1\n", "stock", dir)
	must(t, "2\n", "add", dir, "c", "3")
	must(t, "a 1\nc 3\n", "stock", dir)
}

func TestCLIUsage(t *testing.T) {
	base := t.TempDir()
	dir := filepath.Join(base, "never")
	for _, args := range [][]string{
		{},
		{"dump"},
		{"frobnicate", dir},
		{"append", dir, "add"},
		{"append", dir, "add", "x", "extra"},
		{"dump", dir, "1", "2"},
		{"dump", dir, "-1"},
		{"compact", dir},
		{"compact", dir, "ten"},
		{"add", dir, "sku", "1.5"},
		{"remove", dir, "sku", "-2"},
		{"stock", dir, "extra"},
		{"snapshot"},
	} {
		code, out, errs := run(args...)
		if code != 2 || out != "" || !strings.HasPrefix(errs, "usage:") {
			t.Fatalf("%q: code=%d stdout=%q stderr=%q", args, code, out, errs)
		}
	}
	if _, err := os.Stat(dir); !os.IsNotExist(err) {
		t.Fatalf("a usage error must not create DIR")
	}
}
