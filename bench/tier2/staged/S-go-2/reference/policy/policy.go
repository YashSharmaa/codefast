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
	Algo    string // "bucket" or "window"
	Cost    int    // tokens one matching request takes
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
	if !strings.HasPrefix(p, "/") || strings.ContainsAny(p, "?#") {
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
	if len(fields) < 5 || len(fields) > 9 || fields[0] != "policy" {
		return Policy{}, "bad line"
	}
	p := Policy{Name: fields[1], Pattern: fields[3], By: "ip", Algo: "bucket", Line: n}
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
		case "algo":
			if val != "bucket" && val != "window" {
				return p, "bad option"
			}
			p.Algo = val
		case "cost":
			if p.Cost, ok = positive(val); !ok {
				return p, "bad option"
			}
		default:
			return p, "bad option"
		}
	}
	if p.Algo == "window" && used["burst"] {
		return p, "bad option"
	}
	if p.Burst == 0 {
		p.Burst = p.Limit
	}
	if p.Cost == 0 {
		p.Cost = 1
	}
	if p.Cost > p.Burst {
		return p, "bad option"
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

func hexValue(c byte) (byte, bool) {
	switch {
	case c >= '0' && c <= '9':
		return c - '0', true
	case c >= 'a' && c <= 'f':
		return c - 'a' + 10, true
	case c >= 'A' && c <= 'F':
		return c - 'A' + 10, true
	}
	return 0, false
}

// cleanPath drops the query and fragment and decodes percent escapes,
// except escaped slashes.
func cleanPath(path string) string {
	if i := strings.IndexAny(path, "?#"); i >= 0 {
		path = path[:i]
	}
	var b strings.Builder
	for i := 0; i < len(path); i++ {
		if path[i] == '%' && i+2 < len(path) {
			hi, ok1 := hexValue(path[i+1])
			lo, ok2 := hexValue(path[i+2])
			if c := hi<<4 | lo; ok1 && ok2 && c != '/' {
				b.WriteByte(c)
				i += 2
				continue
			}
		}
		b.WriteByte(path[i])
	}
	return b.String()
}

// MatchPath reports whether path matches pattern.
func MatchPath(pattern, path string) bool {
	path = cleanPath(path)
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
