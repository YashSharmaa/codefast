// Package policy parses rate-limit policy files and matches requests.
package policy

import (
	"fmt"
	"strconv"
	"strings"
	"time"
)

// Policy is one rate-limit rule.
type Policy struct {
	Name    string
	Methods []string // nil means any method
	Pattern string
	Limit   int
	Window  time.Duration
	Burst   int
	By      string // "ip", "user" or "global"
	Line    int
}

// ParseError reports a problem on one line of a policy file.
type ParseError struct {
	Line int
	Msg  string
}

func (e *ParseError) Error() string {
	return fmt.Sprintf("line %d: %s", e.Line, e.Msg)
}

func allChars(s string, ok func(r rune) bool) bool {
	if s == "" {
		return false
	}
	for _, r := range s {
		if !ok(r) {
			return false
		}
	}
	return true
}

func isNameRune(r rune) bool {
	return r == '-' || r == '_' || (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9')
}

func isUpper(r rune) bool { return r >= 'A' && r <= 'Z' }

func isDigit(r rune) bool { return r >= '0' && r <= '9' }

// positive parses a decimal integer written with digits only and greater than zero.
func positive(s string) (int, bool) {
	if !allChars(s, isDigit) {
		return 0, false
	}
	n, err := strconv.Atoi(s)
	if err != nil || n <= 0 {
		return 0, false
	}
	return n, true
}

func validPattern(p string) bool {
	if !strings.HasPrefix(p, "/") {
		return false
	}
	segs := strings.Split(p[1:], "/")
	for i, s := range segs {
		if s == "**" && i != len(segs)-1 {
			return false
		}
		if strings.Contains(s, "*") && s != "*" && s != "**" {
			return false
		}
	}
	return true
}

func parseRate(s string) (int, time.Duration, bool) {
	parts := strings.Split(s, "/")
	if len(parts) != 2 || len(parts[1]) < 2 {
		return 0, 0, false
	}
	limit, ok := positive(parts[0])
	if !ok {
		return 0, 0, false
	}
	w := parts[1]
	n, ok := positive(w[:len(w)-1])
	if !ok {
		return 0, 0, false
	}
	units := map[byte]time.Duration{'s': time.Second, 'm': time.Minute, 'h': time.Hour}
	unit, ok := units[w[len(w)-1]]
	if !ok {
		return 0, 0, false
	}
	return limit, time.Duration(n) * unit, true
}

func parseLine(fields []string, n int, seen map[string]bool) (Policy, string) {
	if len(fields) < 5 || len(fields) > 7 || fields[0] != "policy" {
		return Policy{}, "bad line"
	}
	p := Policy{Name: fields[1], Pattern: fields[3], By: "ip", Line: n}
	if !allChars(p.Name, isNameRune) {
		return p, "bad name"
	}
	if seen[p.Name] {
		return p, fmt.Sprintf("duplicate policy %q", p.Name)
	}
	if fields[2] != "*" {
		for _, m := range strings.Split(fields[2], ",") {
			if !allChars(m, isUpper) {
				return p, "bad methods"
			}
			p.Methods = append(p.Methods, m)
		}
	}
	if !validPattern(p.Pattern) {
		return p, "bad pattern"
	}
	var ok bool
	if p.Limit, p.Window, ok = parseRate(fields[4]); !ok {
		return p, "bad rate"
	}
	used := map[string]bool{}
	for _, opt := range fields[5:] {
		key, val, found := strings.Cut(opt, "=")
		if !found || used[key] {
			return p, "bad option"
		}
		used[key] = true
		switch key {
		case "burst":
			if p.Burst, ok = positive(val); !ok {
				return p, "bad option"
			}
		case "by":
			if val != "ip" && val != "user" && val != "global" {
				return p, "bad option"
			}
			p.By = val
		default:
			return p, "bad option"
		}
	}
	if p.Burst == 0 {
		p.Burst = p.Limit
	}
	return p, ""
}

// Parse reads a policy file. Errors are *ParseError.
func Parse(src string) ([]Policy, error) {
	var out []Policy
	seen := map[string]bool{}
	for i, line := range strings.Split(src, "\n") {
		s := strings.TrimSpace(line)
		if s == "" || strings.HasPrefix(s, "#") {
			continue
		}
		p, msg := parseLine(strings.Fields(s), i+1, seen)
		if msg != "" {
			return nil, &ParseError{Line: i + 1, Msg: msg}
		}
		seen[p.Name] = true
		out = append(out, p)
	}
	return out, nil
}

// MatchPath reports whether path matches pattern.
func MatchPath(pattern, path string) bool {
	if !strings.HasPrefix(path, "/") || !strings.HasPrefix(pattern, "/") {
		return false
	}
	ps := strings.Split(pattern[1:], "/")
	ss := strings.Split(path[1:], "/")
	for i, p := range ps {
		if p == "**" {
			return true
		}
		if i >= len(ss) {
			return false
		}
		if p == "*" {
			if ss[i] == "" {
				return false
			}
		} else if p != ss[i] {
			return false
		}
	}
	return len(ss) == len(ps)
}

// Matches reports whether a request with this method and path is covered by p.
func (p Policy) Matches(method, path string) bool {
	if p.Methods != nil {
		m := strings.ToUpper(method)
		found := false
		for _, x := range p.Methods {
			if x == m {
				found = true
				break
			}
		}
		if !found {
			return false
		}
	}
	return MatchPath(p.Pattern, path)
}
