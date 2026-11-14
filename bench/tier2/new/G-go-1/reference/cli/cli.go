// Package cli implements the buildplan command line.
package cli

import (
	"errors"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"

	"example.com/buildplan/graph"
	"example.com/buildplan/parse"
	"example.com/buildplan/plan"
)

const usage = "usage: buildplan order FILE [GOAL...] | plan FILE MTIMES [GOAL...] | cycles FILE"

// Run executes the command line in args (without the program name).
func Run(args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 {
		fmt.Fprintln(stderr, usage)
		return 2
	}
	switch {
	case args[0] == "order" && len(args) >= 2:
		return runOrder(args[1], args[2:], stdout, stderr)
	case args[0] == "plan" && len(args) >= 3:
		return runPlan(args[1], args[2], args[3:], stdout, stderr)
	case args[0] == "cycles" && len(args) == 2:
		return runCycles(args[1], stdout, stderr)
	}
	fmt.Fprintln(stderr, usage)
	return 2
}

func fail(stderr io.Writer, err error) int {
	fmt.Fprintf(stderr, "error: %s\n", err)
	var ce *graph.CycleError
	var me *plan.MissingError
	if errors.As(err, &ce) || errors.As(err, &me) {
		return 1
	}
	return 2
}

func load(path string) (*parse.File, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return parse.Parse(string(data))
}

func runOrder(path string, goals []string, stdout, stderr io.Writer) int {
	f, err := load(path)
	if err != nil {
		return fail(stderr, err)
	}
	goals, err = plan.DefaultGoals(f, goals)
	if err != nil {
		return fail(stderr, err)
	}
	order, err := graph.FromFile(f).Order(goals)
	if err != nil {
		return fail(stderr, err)
	}
	for _, n := range order {
		fmt.Fprintln(stdout, n)
	}
	return 0
}

func readMtimes(path string) (plan.MapState, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	st := plan.MapState{}
	for i, line := range strings.Split(string(data), "\n") {
		s := strings.TrimSpace(line)
		if s == "" || strings.HasPrefix(s, "#") {
			continue
		}
		fields := strings.Fields(s)
		if len(fields) != 2 {
			return nil, fmt.Errorf("mtimes line %d: bad line", i+1)
		}
		v, err := strconv.ParseInt(fields[1], 10, 64)
		if err != nil {
			return nil, fmt.Errorf("mtimes line %d: bad line", i+1)
		}
		st[fields[0]] = v
	}
	return st, nil
}

func runPlan(path, mtimes string, goals []string, stdout, stderr io.Writer) int {
	f, err := load(path)
	if err != nil {
		return fail(stderr, err)
	}
	st, err := readMtimes(mtimes)
	if err != nil {
		return fail(stderr, err)
	}
	steps, err := plan.Make(f, goals, st)
	if err != nil {
		return fail(stderr, err)
	}
	if len(steps) == 0 {
		fmt.Fprintln(stdout, "nothing to do")
		return 0
	}
	for _, s := range steps {
		fmt.Fprintf(stdout, "%s: %s\n", s.Target, s.Reason)
		for _, c := range s.Commands {
			fmt.Fprintf(stdout, "  %s\n", c)
		}
	}
	return 0
}

func runCycles(path string, stdout, stderr io.Writer) int {
	f, err := load(path)
	if err != nil {
		return fail(stderr, err)
	}
	cycles := graph.FromFile(f).Cycles()
	if len(cycles) == 0 {
		fmt.Fprintln(stdout, "no cycles")
		return 0
	}
	for _, c := range cycles {
		fmt.Fprintln(stdout, strings.Join(c, " "))
	}
	return 1
}
