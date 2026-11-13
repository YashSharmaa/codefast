package server

import (
	"path"
	"testing"

	"example.com/app/config"
)

var cfg *config.Config

var timeout = 30

type fakeClock struct{ now int64 }

func (f *fakeClock) Now() int64 { return f.now }

func (f *fakeClock) TestHelperMethod() {}

func TestMain(m *testing.M) { m.Run() }

func TestRun(t *testing.T) {
	type args struct {
		name string
		opts *config.Options
	}
	const retries = 3
	var clock fakeClock
	tests := []struct {
		name    string
		args    args
		want    int
		wantErr bool
	}{
		{name: "empty", args: args{name: "a"}, want: retries},
		{name: "full", args: args{opts: &config.Options{Debug: true}}, wantErr: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := run(tt.args.name, tt.args.opts)
			if (err != nil) != tt.wantErr {
				t.Fatalf("run() error = %v", err)
			}
			if got != tt.want {
				t.Errorf("got %d at %d", got, clock.Now())
			}
		})
	}
}

func TestShadow(t *testing.T) {
	cfg.Load("a")
	timeout := timeout + 1
	path := path.Join("a", "b")
	_ = path
	if cfg := config.New(); cfg != nil {
		cfg.Load("b")
	} else if alt := config.Default; alt.Debug {
		alt.Reset()
	}
	cfg.Validate()
	c := &fakeClock{}
	{
		c := config.Options{}
		c.Apply(timeout)
	}
	c.Now()
	var testing = t
	testing.Helper()
	for i, cfg := 0, cfg; i < timeout; i++ {
		cfg.Step(i)
	}
	switch cfg := cfg.Clone(); {
	case cfg.Debug:
		cfg.Load("c")
	default:
		t := cfg.Trace
		t.Stop()
	}
	t.Log(cfg.Name)
}

func BenchmarkRun(b *testing.B) {
	for i := 0; i < b.N; i++ {
		run("x", nil)
	}
	for range b.N {
		run("y", nil)
	}
	var total int
	var first, last *fakeClock
	for total = range timeout {
		first.Now()
	}
	last.now = int64(total)
}

func FuzzRun(f *testing.F) {
	f.Fuzz(func(t *testing.T, name string) {
		if _, err := run(name, nil); err != nil {
			t.Skip()
		}
	})
}

func ExampleServer() { run("example", nil) }

func Example_withOptions() {}

func Testify(x int) int { return x }

func testHelper(t *testing.T) (c *fakeClock, cleanup func()) {
	c = &fakeClock{now: 1}
	cleanup = func() { c.now = 0 }
retry:
	for {
		select {
		case <-done:
			break retry
		default:
			goto retry
		}
	}
	return c, cleanup
}

func run(name string, opts *config.Options) (n int, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = r.(error)
			n = opts.Limit
		}
	}()
	return len(name), nil
}
