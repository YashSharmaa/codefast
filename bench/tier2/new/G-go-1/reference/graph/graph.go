// Package graph holds the dependency graph and its orderings.
package graph

import (
	"sort"
	"strings"

	"example.com/buildplan/parse"
)

// Graph is a directed graph where an edge from A to B means A depends on B.
type Graph struct {
	nodes []string
	deps  map[string][]string
}

// New returns an empty graph.
func New() *Graph {
	return &Graph{deps: map[string][]string{}}
}

// AddNode adds a node if it is not present.
func (g *Graph) AddNode(name string) {
	if _, ok := g.deps[name]; ok {
		return
	}
	g.nodes = append(g.nodes, name)
	g.deps[name] = nil
}

// AddEdge records that from depends on to.
func (g *Graph) AddEdge(from, to string) {
	g.AddNode(from)
	g.AddNode(to)
	for _, d := range g.deps[from] {
		if d == to {
			return
		}
	}
	g.deps[from] = append(g.deps[from], to)
}

// Nodes returns the nodes in insertion order.
func (g *Graph) Nodes() []string {
	return append([]string(nil), g.nodes...)
}

// Deps returns the dependencies of a node in insertion order.
func (g *Graph) Deps(name string) []string {
	return append([]string(nil), g.deps[name]...)
}

// FromFile builds the graph of a parsed build file.
func FromFile(f *parse.File) *Graph {
	g := New()
	for _, t := range f.Targets {
		g.AddNode(t.Name)
		for _, d := range t.Deps {
			g.AddEdge(t.Name, d)
		}
	}
	return g
}

// CycleError reports a dependency cycle met during Order.
type CycleError struct {
	Path []string
}

func (e *CycleError) Error() string {
	return "cycle: " + strings.Join(e.Path, " -> ")
}

// Order returns every node reachable from goals, dependencies first.
func (g *Graph) Order(goals []string) ([]string, error) {
	done := map[string]bool{}
	onStack := map[string]bool{}
	var stack, out []string
	var visit func(n string) error
	visit = func(n string) error {
		if done[n] {
			return nil
		}
		if onStack[n] {
			i := len(stack) - 1
			for stack[i] != n {
				i--
			}
			path := append(append([]string(nil), stack[i:]...), n)
			return &CycleError{Path: path}
		}
		onStack[n] = true
		stack = append(stack, n)
		for _, d := range g.deps[n] {
			if err := visit(d); err != nil {
				return err
			}
		}
		stack = stack[:len(stack)-1]
		onStack[n] = false
		done[n] = true
		out = append(out, n)
		return nil
	}
	for _, goal := range goals {
		if err := visit(goal); err != nil {
			return nil, err
		}
	}
	return out, nil
}

// Cycles returns every strongly connected component that holds a cycle.
func (g *Graph) Cycles() [][]string {
	index := map[string]int{}
	low := map[string]int{}
	onStack := map[string]bool{}
	var stack []string
	var result [][]string
	next := 0
	var strong func(v string)
	strong = func(v string) {
		index[v] = next
		low[v] = next
		next++
		stack = append(stack, v)
		onStack[v] = true
		for _, w := range g.deps[v] {
			if _, seen := index[w]; !seen {
				strong(w)
				low[v] = min(low[v], low[w])
			} else if onStack[w] {
				low[v] = min(low[v], index[w])
			}
		}
		if low[v] != index[v] {
			return
		}
		var comp []string
		for {
			w := stack[len(stack)-1]
			stack = stack[:len(stack)-1]
			onStack[w] = false
			comp = append(comp, w)
			if w == v {
				break
			}
		}
		if len(comp) > 1 || g.hasSelfLoop(v) {
			sort.Strings(comp)
			result = append(result, comp)
		}
	}
	for _, n := range g.nodes {
		if _, seen := index[n]; !seen {
			strong(n)
		}
	}
	sort.Slice(result, func(i, j int) bool { return result[i][0] < result[j][0] })
	return result
}

func (g *Graph) hasSelfLoop(n string) bool {
	for _, d := range g.deps[n] {
		if d == n {
			return true
		}
	}
	return false
}
