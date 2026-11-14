# throttle: a rate-limit policy engine with an HTTP surface

throttle reads a file of rate-limit policies, keeps one token bucket per policy and key value, and answers whether a request may proceed. It is a Go library with an HTTP handler. Time comes from an injected clock so behaviour is exact and testable. The project will be checked by tests written against the interface below, so names, signatures, messages and output formats must match exactly.

## Module and packages

Module path `example.com/throttle`, `go 1.22` in go.mod, standard library only. Packages: `clock`, `policy`, `bucket`, `engine`, `server`.

## Package clock

```go
type Clock interface { Now() time.Time }
type Real struct{}                  // Now returns time.Now()
type Fake struct { /* unexported */ } // safe for concurrent use
func NewFake(t time.Time) *Fake
func (f *Fake) Now() time.Time
func (f *Fake) Advance(d time.Duration)
```

## Package policy

```go
type Policy struct {
    Name    string
    Methods []string      // nil when the file says "*"
    Pattern string
    Limit   int
    Window  time.Duration
    Burst   int           // the limit when not given
    By      string        // "ip" (default), "user" or "global"
    Line    int           // line number of the policy in the file
}
type ParseError struct { Line int; Msg string }
func (e *ParseError) Error() string // "line N: MSG"
func Parse(src string) ([]Policy, error) // policies in file order, errors are *ParseError
func MatchPath(pattern, path string) bool
func (p Policy) Matches(method, path string) bool
```

`Policy` has exactly these fields, so tests may compare whole values.

The text is split on `\n` and lines are numbered from 1. A line that is blank or whose first non-blank character is `#` is ignored. Every other line is split on whitespace into fields: `policy NAME METHODS PATTERN LIMIT/WINDOW [OPTION] [OPTION]`. The checks below run in this order and the first failure is reported, and parsing stops at the first bad line.

1. Fewer than 5 or more than 7 fields, or a first field other than `policy`: `bad line`.
2. NAME made of ASCII letters, digits, `-` and `_` only, else `bad name`. A NAME used on an earlier line: `duplicate policy "NAME"` (Go `%q` quoting).
3. METHODS is `*` (any method) or a comma-separated list of non-empty names made only of ASCII uppercase letters, such as `GET` or `GET,POST`, else `bad methods`.
4. PATTERN starts with `/`. After the leading `/` it is split on `/` into segments. A segment that contains `*` must be exactly `*` or `**`, and `**` may only be the last segment, else `bad pattern`.
5. LIMIT is a positive decimal integer of digits only. WINDOW is a positive decimal integer of digits only followed by `s`, `m` or `h` (seconds, minutes, hours), for example `30s`, `1m`, `2h`. Anything else: `bad rate`.
6. Each OPTION is `burst=N` (N a positive decimal integer of digits only) or `by=ip`, `by=user` or `by=global`. An unknown option, a bad value, or the same option twice: `bad option`.

Path matching: a path that does not start with `/` matches nothing. Pattern and path are both split into segments the same way (so `/` is one empty segment and `/a/` is `a` plus an empty segment). Segments are compared in turn: a literal segment must be equal (case-sensitive), `*` matches exactly one non-empty segment, and a final `**` matches all remaining path segments, including none (so `/api/**` matches `/api`, `/api/` and `/api/v1/users`). Without `**` the segment counts must be equal. The request method is converted to upper case before it is compared with METHODS, so `get` matches `GET`.

## Package bucket

```go
type Bucket struct { /* unexported */ }
func New(limit int, window time.Duration, burst int, now time.Time) *Bucket
func (b *Bucket) Available(now time.Time) int
func (b *Bucket) Take(now time.Time) bool
func (b *Bucket) RetryAfter(now time.Time) int
```

All arithmetic is exact integer arithmetic. Let W be the window in nanoseconds. A bucket holds a level counted in units, where one token is W units. It is created full, with a level of burst × W, and its refill time set to `now`. Each method first refills: if `now` is before the refill time nothing happens, otherwise the level becomes the smaller of burst × W and level + limit × E, where E is the nanoseconds since the refill time, and the refill time becomes `now`. (So limit tokens are added per window.) The computation must not overflow for windows up to 24h, limits and bursts up to 100000 and gaps up to 1000 hours. After refilling:

- `Available` returns floor(level / W), the whole tokens.
- `Take` removes one token (W units) and returns true when level ≥ W, otherwise it changes nothing and returns false.
- `RetryAfter` returns 0 when level ≥ W. Otherwise it returns S = ceil(N / 1e9), where N = ceil((W − level) / limit) is the nanoseconds until one whole token is there, and returns 1 instead when S is less than 1.

## Package engine

```go
type Request struct { Method, Path, User, IP string }
type Decision struct {
    Allowed    bool
    Matched    []string // names of matching policies in file order, empty (not nil) when none
    Denied     string   // first denying policy in file order, "" when allowed
    RetryAfter int      // 0 when allowed
    Remaining  int
}
type Engine struct { /* unexported */ }
func New(policies []policy.Policy, c clock.Clock) *Engine
func (e *Engine) Policies() []policy.Policy // file order
func (e *Engine) Check(r Request) Decision
```

`Decision` has exactly these fields, so tests may compare whole values.

`Check` is safe for concurrent use and reads the clock once. Every policy that matches the method and path is checked. Each policy keeps one bucket per key value: the IP for `by=ip`, the user for `by=user` (an empty user counts as `anonymous`), and one shared key for `by=global`. A bucket is created the first time its policy and key are seen, at that check's time. A matching policy denies when its bucket has no whole token available. The request is allowed only if no matching policy denies, and only then is one token taken from the bucket of every matching policy. A denied request takes nothing. `RetryAfter` is the largest `RetryAfter` among the denying buckets. `Remaining` is the smallest `Available` among the matching buckets after the decision (after the tokens were taken), or -1 when no policy matched. A request that matches no policy is allowed.

## Package server

```go
func NewHandler(e *engine.Engine) http.Handler
```

Every response has `Content-Type: application/json` and a body that is one JSON value followed by a newline, with keys in the order shown and no extra spaces (what `json.NewEncoder(w).Encode` writes for a struct with these fields).

- `POST /check`: the body must be a JSON object with string fields `method`, `path`, `user`, `ip` (unknown fields ignored, `user` and `ip` may be missing). A body that is not one valid JSON object (including `null` and arrays), a field of the wrong type, or an empty or missing `method` or `path` gives 400 with `{"error":"bad request"}`. Otherwise the answer is 200 when allowed and 429 when denied, with the body `{"allowed":false,"matched":["a","b"],"denied":"a","retry_after":3,"remaining":0}` built from the Decision. A 429 also has the header `Retry-After` set to the decimal RetryAfter.
- `GET /policies`: 200 with a JSON array, in file order, of `{"name":"api","methods":"GET,POST","pattern":"/api/**","limit":10,"window_seconds":60,"burst":10,"by":"ip"}` where `methods` is `*` or the method names joined by `,`. With no policies the body is `[]`.
- Any other method on these two paths: 405 with `{"error":"method not allowed"}` and an `Allow` header of `POST` or `GET`. Any other path: 404 with `{"error":"not found"}`.

## Constraints

Go 1.22 or later, standard library only, no network, and no test may need a real listening socket or real waiting. Your own tests may live anywhere in the module except files named `hidden_*_test.go`, which are reserved for the checking tests.
