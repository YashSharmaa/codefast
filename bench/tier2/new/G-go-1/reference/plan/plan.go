// Package plan decides which targets must run and why.
package plan

import (
	"errors"
	"fmt"

	"example.com/buildplan/graph"
	"example.com/buildplan/parse"
)

// State reports the modification time of a file, if it exists.
type State interface {
	ModTime(name string) (int64, bool)
}

// MapState is a State backed by a map.
type MapState map[string]int64

// ModTime implements State.
func (m MapState) ModTime(name string) (int64, bool) {
	t, ok := m[name]
	return t, ok
}

// Step is one target that must run.
type Step struct {
	Target   string
	Reason   string
	Commands []string
}

// MissingError reports a name that is neither a target nor an existing file.
type MissingError struct {
	Name     string
	NeededBy string
}

func (e *MissingError) Error() string {
	if e.NeededBy == "" {
		return fmt.Sprintf("missing %q", e.Name)
	}
	return fmt.Sprintf("missing %q needed by %q", e.Name, e.NeededBy)
}

// ErrNoTargets is returned when no goal is given and the file declares no target.
var ErrNoTargets = errors.New("no targets")

// DefaultGoals returns goals, or the first declared target when goals is empty.
func DefaultGoals(f *parse.File, goals []string) ([]string, error) {
	if len(goals) > 0 {
		return goals, nil
	}
	if len(f.Targets) == 0 {
		return nil, ErrNoTargets
	}
	return []string{f.Targets[0].Name}, nil
}

// Make returns the steps needed to bring goals up to date, in run order.
func Make(f *parse.File, goals []string, st State) ([]Step, error) {
	goals, err := DefaultGoals(f, goals)
	if err != nil {
		return nil, err
	}
	order, err := graph.FromFile(f).Order(goals)
	if err != nil {
		return nil, err
	}
	rebuilt := map[string]bool{}
	var steps []Step
	for _, name := range order {
		t := f.Target(name)
		if t == nil {
			if _, ok := st.ModTime(name); !ok {
				return nil, &MissingError{Name: name, NeededBy: neededBy(f, order, name)}
			}
			continue
		}
		reason := why(t, st, rebuilt)
		if reason == "" {
			continue
		}
		rebuilt[name] = true
		steps = append(steps, Step{Target: name, Reason: reason, Commands: append([]string(nil), t.Commands...)})
	}
	return steps, nil
}

func why(t *parse.Target, st State, rebuilt map[string]bool) string {
	own, ok := st.ModTime(t.Name)
	if !ok {
		return "missing"
	}
	for _, d := range t.Deps {
		if rebuilt[d] {
			return fmt.Sprintf("dependency %s is being rebuilt", d)
		}
	}
	for _, d := range t.Deps {
		if m, ok := st.ModTime(d); ok && m > own {
			return fmt.Sprintf("dependency %s is newer", d)
		}
	}
	return ""
}

func neededBy(f *parse.File, order []string, name string) string {
	for _, n := range order {
		if t := f.Target(n); t != nil {
			for _, d := range t.Deps {
				if d == name {
					return n
				}
			}
		}
	}
	return ""
}
