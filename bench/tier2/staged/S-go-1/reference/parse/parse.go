// Package parse reads the small Makefile-like build file format.
package parse

import (
	"fmt"
	"strings"
)

// Target is one rule of the build file.
type Target struct {
	Name     string
	Deps     []string
	Commands []string
	Line     int
}

// Rule is a pattern rule such as "%.o: %.c".
type Rule struct {
	Pattern  string
	Deps     []string
	Commands []string
	Line     int
}

// File is a parsed build file.
type File struct {
	Vars    map[string]string
	Targets []*Target
	Rules   []*Rule
	Phony   []string
}

// Target returns the declared target with the given name, or nil.
func (f *File) Target(name string) *Target {
	for _, t := range f.Targets {
		if t.Name == name {
			return t
		}
	}
	return nil
}

// rule returns the pattern rule with the given pattern, or nil.
func (f *File) rule(pattern string) *Rule {
	for _, r := range f.Rules {
		if r.Pattern == pattern {
			return r
		}
	}
	return nil
}

// stem returns the text that % stands for when pattern matches name.
func stem(pattern, name string) (string, bool) {
	i := strings.IndexByte(pattern, '%')
	prefix, suffix := pattern[:i], pattern[i+1:]
	if len(name) <= len(prefix)+len(suffix) || !strings.HasPrefix(name, prefix) || !strings.HasSuffix(name, suffix) {
		return "", false
	}
	return name[len(prefix) : len(name)-len(suffix)], true
}

// Resolve returns the declared target of that name, or a target made from
// the first pattern rule that matches it, or nil.
func (f *File) Resolve(name string) *Target {
	if t := f.Target(name); t != nil {
		return t
	}
	for _, r := range f.Rules {
		s, ok := stem(r.Pattern, name)
		if !ok {
			continue
		}
		t := &Target{Name: name, Line: r.Line}
		var deps []string
		for _, d := range r.Deps {
			deps = append(deps, strings.ReplaceAll(d, "%", s))
		}
		t.Deps = unique(deps)
		for _, c := range r.Commands {
			t.Commands = append(t.Commands, automatic(c, t, s))
		}
		return t
	}
	return nil
}

func unique(names []string) []string {
	var out []string
	seen := map[string]bool{}
	for _, d := range names {
		if !seen[d] {
			seen[d] = true
			out = append(out, d)
		}
	}
	return out
}

// automatic replaces $@, $<, $^ and $* in one left-to-right pass.
func automatic(cmd string, t *Target, stem string) string {
	var b strings.Builder
	for i := 0; i < len(cmd); i++ {
		if cmd[i] == '$' && i+1 < len(cmd) {
			switch cmd[i+1] {
			case '@':
				b.WriteString(t.Name)
				i++
				continue
			case '<':
				if len(t.Deps) > 0 {
					b.WriteString(t.Deps[0])
				}
				i++
				continue
			case '^':
				b.WriteString(strings.Join(t.Deps, " "))
				i++
				continue
			case '*':
				b.WriteString(stem)
				i++
				continue
			}
		}
		b.WriteByte(cmd[i])
	}
	return b.String()
}

// ParseError reports a problem on one line of the build file.
type ParseError struct {
	Line int
	Msg  string
}

func (e *ParseError) Error() string {
	return fmt.Sprintf("line %d: %s", e.Line, e.Msg)
}

func isNameStart(c byte) bool {
	return c == '_' || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
}

func isNameChar(c byte) bool {
	return isNameStart(c) || (c >= '0' && c <= '9')
}

// assignment reports whether line is NAME = value or NAME += value and
// returns its parts.
func assignment(line string) (string, string, bool, bool) {
	if len(line) == 0 || !isNameStart(line[0]) {
		return "", "", false, false
	}
	i := 1
	for i < len(line) && isNameChar(line[i]) {
		i++
	}
	name := line[:i]
	rest := strings.TrimLeft(line[i:], " \t")
	if strings.HasPrefix(rest, "+=") {
		return name, strings.TrimSpace(rest[2:]), true, true
	}
	if !strings.HasPrefix(rest, "=") {
		return "", "", false, false
	}
	return name, strings.TrimSpace(rest[1:]), false, true
}

func expand(s string, vars map[string]string, line int) (string, error) {
	var b strings.Builder
	i := 0
	for i < len(s) {
		if strings.HasPrefix(s[i:], "$(") {
			j := i + 2
			if j < len(s) && isNameStart(s[j]) {
				k := j + 1
				for k < len(s) && isNameChar(s[k]) {
					k++
				}
				if k < len(s) && s[k] == ')' {
					name := s[j:k]
					v, ok := vars[name]
					if !ok {
						return "", &ParseError{Line: line, Msg: fmt.Sprintf("undefined variable %q", name)}
					}
					b.WriteString(v)
					i = k + 1
					continue
				}
			}
		}
		b.WriteByte(s[i])
		i++
	}
	return b.String(), nil
}

// stripComment removes everything from the first # and trims the rest.
func stripComment(line string) string {
	if i := strings.IndexByte(line, '#'); i >= 0 {
		line = line[:i]
	}
	return strings.TrimSpace(line)
}

// Parse parses the text of a build file.
func Parse(src string) (*File, error) {
	f := &File{Vars: map[string]string{}}
	var cur *[]string
	inPhony := false // the most recent target line was .PHONY
	phonyLine := map[string]int{}
	// For a target, the line its commands follow, so a second line of the
	// same target cannot add commands too.
	var curTarget *Target
	var curLine int
	cmdLine := map[*Target]int{}
	for idx, raw := range strings.Split(src, "\n") {
		n := idx + 1
		line := strings.TrimSuffix(raw, "\r")
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}
		if line[0] == ' ' || line[0] == '\t' {
			if cur == nil {
				if inPhony {
					return nil, &ParseError{Line: n, Msg: "commands for .PHONY"}
				}
				return nil, &ParseError{Line: n, Msg: "command before any target"}
			}
			if curTarget != nil {
				if first, ok := cmdLine[curTarget]; ok && first != curLine {
					return nil, &ParseError{Line: n, Msg: fmt.Sprintf("duplicate commands for %q", curTarget.Name)}
				}
				cmdLine[curTarget] = curLine
			}
			cmd, err := expand(trimmed, f.Vars, n)
			if err != nil {
				return nil, err
			}
			*cur = append(*cur, cmd)
			continue
		}
		trimmed = stripComment(trimmed)
		if name, value, add, ok := assignment(trimmed); ok {
			v, err := expand(value, f.Vars, n)
			if err != nil {
				return nil, err
			}
			if old := f.Vars[name]; add && old != "" {
				if v == "" {
					v = old
				} else {
					v = old + " " + v
				}
			}
			f.Vars[name] = v
			continue
		}
		colon := strings.IndexByte(trimmed, ':')
		if colon < 0 {
			return nil, &ParseError{Line: n, Msg: "bad line"}
		}
		name, err := expand(strings.TrimSpace(trimmed[:colon]), f.Vars, n)
		if err != nil {
			return nil, err
		}
		if name == "" || strings.ContainsAny(name, " \t") {
			return nil, &ParseError{Line: n, Msg: "bad line"}
		}
		if name == ".PHONY" {
			depText, err := expand(trimmed[colon+1:], f.Vars, n)
			if err != nil {
				return nil, err
			}
			for _, d := range strings.Fields(depText) {
				if _, seen := phonyLine[d]; !seen {
					phonyLine[d] = n
					f.Phony = append(f.Phony, d)
				}
			}
			cur, curTarget = nil, nil
			inPhony = true
			continue
		}
		inPhony = false
		if strings.Contains(name, "%") {
			if strings.Count(name, "%") != 1 {
				return nil, &ParseError{Line: n, Msg: "bad line"}
			}
			if f.rule(name) != nil {
				return nil, &ParseError{Line: n, Msg: fmt.Sprintf("duplicate rule %q", name)}
			}
			depText, err := expand(trimmed[colon+1:], f.Vars, n)
			if err != nil {
				return nil, err
			}
			r := &Rule{Pattern: name, Deps: unique(strings.Fields(depText)), Line: n}
			f.Rules = append(f.Rules, r)
			cur, curTarget = &r.Commands, nil
			continue
		}
		depText, err := expand(trimmed[colon+1:], f.Vars, n)
		if err != nil {
			return nil, err
		}
		t := f.Target(name)
		if t == nil {
			t = &Target{Name: name, Line: n}
			f.Targets = append(f.Targets, t)
		}
		t.Deps = unique(append(t.Deps, strings.Fields(depText)...))
		cur, curTarget, curLine = &t.Commands, t, n
	}
	for _, p := range f.Phony {
		if f.Target(p) == nil {
			return nil, &ParseError{Line: phonyLine[p], Msg: fmt.Sprintf("phony %q is not a target", p)}
		}
	}
	for _, t := range f.Targets {
		for i, c := range t.Commands {
			t.Commands[i] = automatic(c, t, "")
		}
	}
	return f, nil
}
