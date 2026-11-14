// Package sched groups the steps of a plan into waves that can run in parallel.
package sched

import (
	"example.com/buildplan/parse"
	"example.com/buildplan/plan"
)

// Waves places each step in the lowest wave after the waves of its
// dependencies that has room, with at most jobs steps per wave when jobs > 0.
func Waves(f *parse.File, steps []plan.Step, jobs int) [][]string {
	wave := map[string]int{}
	waves := [][]string{}
	for _, s := range steps {
		w := 1
		if t := f.Resolve(s.Target); t != nil {
			for _, d := range t.Deps {
				if dw, ok := wave[d]; ok && dw >= w {
					w = dw + 1
				}
			}
		}
		for jobs > 0 && w <= len(waves) && len(waves[w-1]) >= jobs {
			w++
		}
		for len(waves) < w {
			waves = append(waves, nil)
		}
		waves[w-1] = append(waves[w-1], s.Target)
		wave[s.Target] = w
	}
	return waves
}

// Critical returns the longest chain of steps in which each step depends on
// the one before it, found from the waves without a limit.
func Critical(f *parse.File, steps []plan.Step) []string {
	waves := Waves(f, steps, 0)
	if len(waves) == 0 {
		return []string{}
	}
	wave := map[string]int{}
	for i, names := range waves {
		for _, n := range names {
			wave[n] = i + 1
		}
	}
	cur := waves[len(waves)-1][0]
	path := []string{cur}
	for w := len(waves); w > 1; w-- {
		if t := f.Resolve(cur); t != nil {
			for _, d := range t.Deps {
				if wave[d] == w-1 {
					cur = d
					break
				}
			}
		}
		path = append(path, cur)
	}
	for i, j := 0, len(path)-1; i < j; i, j = i+1, j-1 {
		path[i], path[j] = path[j], path[i]
	}
	return path
}
