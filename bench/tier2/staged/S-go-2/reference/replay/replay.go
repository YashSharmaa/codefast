// Package replay runs a request log against a set of policies.
package replay

import (
	"fmt"
	"strconv"
	"strings"
	"time"

	"example.com/throttle/clock"
	"example.com/throttle/engine"
	"example.com/throttle/policy"
)

// LineError reports a problem on one line of a log.
type LineError struct {
	Line int
	Msg  string
}

func (e *LineError) Error() string {
	return fmt.Sprintf("line %d: %s", e.Line, e.Msg)
}

// Result is the decision for one logged request.
type Result struct {
	Line     int
	Decision engine.Decision
}

// Summary counts what one policy did during a replay.
type Summary struct {
	Name    string
	Matched int
	Denied  int
}

// Report is the outcome of a replay.
type Report struct {
	Requests int
	Allowed  int
	Denied   int
	Policies []Summary
	Results  []Result
}

type event struct {
	line   int
	offset int64
	req    engine.Request
}

func isDigits(s string) bool {
	if s == "" {
		return false
	}
	for _, r := range s {
		if r < '0' || r > '9' {
			return false
		}
	}
	return true
}

func parse(log string) ([]event, error) {
	var out []event
	last := int64(0)
	for i, raw := range strings.Split(log, "\n") {
		n := i + 1
		s := strings.TrimSpace(raw)
		if s == "" || strings.HasPrefix(s, "#") {
			continue
		}
		fields := strings.Fields(s)
		if len(fields) < 3 || len(fields) > 5 {
			return nil, &LineError{n, "bad line"}
		}
		ev := event{line: n, req: engine.Request{Method: fields[1], Path: fields[2]}}
		seen := map[string]bool{}
		for _, opt := range fields[3:] {
			key, val, ok := strings.Cut(opt, "=")
			if !ok || seen[key] || (key != "user" && key != "ip") {
				return nil, &LineError{n, "bad line"}
			}
			seen[key] = true
			if key == "user" {
				ev.req.User = val
			} else {
				ev.req.IP = val
			}
		}
		off, err := strconv.ParseInt(fields[0], 10, 64)
		if !isDigits(fields[0]) || err != nil {
			return nil, &LineError{n, "bad offset"}
		}
		if off < last {
			return nil, &LineError{n, "offset goes back"}
		}
		last = off
		ev.offset = off
		out = append(out, ev)
	}
	return out, nil
}

// Run replays log against policies with a fresh engine and a fake clock.
func Run(policies []policy.Policy, log string) (*Report, error) {
	events, err := parse(log)
	if err != nil {
		return nil, err
	}
	start := time.Date(2000, 1, 1, 0, 0, 0, 0, time.UTC)
	c := clock.NewFake(start)
	e := engine.New(policies, c)
	rep := &Report{Policies: []Summary{}, Results: []Result{}}
	index := map[string]int{}
	for i, p := range policies {
		rep.Policies = append(rep.Policies, Summary{Name: p.Name})
		index[p.Name] = i
	}
	elapsed := time.Duration(0)
	for _, ev := range events {
		at := time.Duration(ev.offset) * time.Millisecond
		c.Advance(at - elapsed)
		elapsed = at
		d := e.Check(ev.req)
		rep.Requests++
		if d.Allowed {
			rep.Allowed++
		} else {
			rep.Denied++
			rep.Policies[index[d.Denied]].Denied++
		}
		for _, name := range d.Matched {
			rep.Policies[index[name]].Matched++
		}
		rep.Results = append(rep.Results, Result{Line: ev.line, Decision: d})
	}
	return rep, nil
}
