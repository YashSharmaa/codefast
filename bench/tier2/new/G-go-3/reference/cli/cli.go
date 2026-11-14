// Package cli implements the journal command line.
package cli

import (
	"fmt"
	"io"
	"strconv"
	"strings"

	"example.com/journal/inventory"
	"example.com/journal/journal"
	"example.com/journal/record"
)

const usage = "usage: journal append DIR TYPE PAYLOAD | dump DIR [FROM] | compact DIR UPTO | add DIR SKU QTY | remove DIR SKU QTY | stock DIR | snapshot DIR"

var arity = map[string][2]int{
	"append": {3, 3}, "dump": {1, 2}, "compact": {2, 2},
	"add": {3, 3}, "remove": {3, 3}, "stock": {1, 1}, "snapshot": {1, 1},
}

func number(s string) (uint64, bool) {
	if s == "" || strings.Trim(s, "0123456789") != "" {
		return 0, false
	}
	n, err := strconv.ParseUint(s, 10, 63)
	return n, err == nil
}

// Run executes the command line in args (without the program name).
func Run(args []string, stdout, stderr io.Writer) int {
	if len(args) < 2 {
		fmt.Fprintln(stderr, usage)
		return 2
	}
	cmd, dir, rest := args[0], args[1], args[2:]
	n, ok := arity[cmd]
	if !ok || len(rest)+1 < n[0] || len(rest)+1 > n[1] {
		fmt.Fprintln(stderr, usage)
		return 2
	}
	var num uint64
	switch {
	case cmd == "dump" && len(rest) == 1, cmd == "compact":
		num, ok = number(rest[0])
	case cmd == "add" || cmd == "remove":
		num, ok = number(rest[1])
	}
	if !ok {
		fmt.Fprintln(stderr, usage)
		return 2
	}
	j, err := journal.Open(dir, journal.Options{})
	if err != nil {
		fmt.Fprintf(stderr, "error: %v\n", err)
		return 1
	}
	defer j.Close()
	if err := run(j, cmd, rest, num, stdout); err != nil {
		fmt.Fprintf(stderr, "error: %v\n", err)
		return 1
	}
	return 0
}

func run(j *journal.Journal, cmd string, rest []string, num uint64, stdout io.Writer) error {
	switch cmd {
	case "append":
		seq, err := j.Append(rest[0], []byte(rest[1]))
		if err == nil {
			fmt.Fprintln(stdout, seq)
		}
		return err
	case "dump":
		return j.Replay(num, func(r record.Record) error {
			fmt.Fprintf(stdout, "%d %s %q\n", r.Seq, r.Type, r.Payload)
			return nil
		})
	case "compact":
		removed, err := j.Compact(num)
		if err == nil {
			fmt.Fprintf(stdout, "removed %d\n", removed)
		}
		return err
	}
	s, err := inventory.Load(j)
	if err != nil {
		return err
	}
	var seq uint64
	switch cmd {
	case "add":
		seq, err = s.Add(rest[0], int(num))
	case "remove":
		seq, err = s.Remove(rest[0], int(num))
	case "snapshot":
		if seq, err = s.Snapshot(); err == nil {
			fmt.Fprintf(stdout, "snapshot %d\n", seq)
		}
		return err
	case "stock":
		for _, it := range s.Inventory().Items() {
			fmt.Fprintf(stdout, "%s %d\n", it.SKU, it.Qty)
		}
		return nil
	}
	if err == nil {
		fmt.Fprintln(stdout, seq)
	}
	return err
}
