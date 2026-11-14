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

// File is a parsed build file.
type File struct {
	Vars    map[string]string
	Targets []*Target
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

// assignment reports whether line is NAME = value and returns its parts.
func assignment(line string) (string, string, bool) {
	if len(line) == 0 || !isNameStart(line[0]) {
		return "", "", false
	}
	i := 1
	for i < len(line) && isNameChar(line[i]) {
		i++
	}
	name := line[:i]
	rest := strings.TrimLeft(line[i:], " \t")
	if !strings.HasPrefix(rest, "=") {
		return "", "", false
	}
	return name, strings.TrimSpace(rest[1:]), true
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

// Parse parses the text of a build file.
func Parse(src string) (*File, error) {
	f := &File{Vars: map[string]string{}}
	var cur *Target
	for idx, raw := range strings.Split(src, "\n") {
		n := idx + 1
		line := strings.TrimSuffix(raw, "\r")
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}
		if line[0] == ' ' || line[0] == '\t' {
			if cur == nil {
				return nil, &ParseError{Line: n, Msg: "command before any target"}
			}
			cmd, err := expand(trimmed, f.Vars, n)
			if err != nil {
				return nil, err
			}
			cur.Commands = append(cur.Commands, cmd)
			continue
		}
		if name, value, ok := assignment(trimmed); ok {
			v, err := expand(value, f.Vars, n)
			if err != nil {
				return nil, err
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
		if f.Target(name) != nil {
			return nil, &ParseError{Line: n, Msg: fmt.Sprintf("duplicate target %q", name)}
		}
		depText, err := expand(trimmed[colon+1:], f.Vars, n)
		if err != nil {
			return nil, err
		}
		t := &Target{Name: name, Line: n}
		seen := map[string]bool{}
		for _, d := range strings.Fields(depText) {
			if !seen[d] {
				seen[d] = true
				t.Deps = append(t.Deps, d)
			}
		}
		f.Targets = append(f.Targets, t)
		cur = t
	}
	return f, nil
}
