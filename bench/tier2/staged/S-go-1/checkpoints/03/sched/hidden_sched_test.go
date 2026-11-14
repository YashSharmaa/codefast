package sched_test

import (
	"reflect"
	"testing"

	"example.com/buildplan/parse"
	"example.com/buildplan/plan"
	"example.com/buildplan/sched"
)

func file(t *testing.T, src string) *parse.File {
	t.Helper()
	f, err := parse.Parse(src)
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}
	return f
}

func steps(names ...string) []plan.Step {
	out := []plan.Step{}
	for _, n := range names {
		out = append(out, plan.Step{Target: n, Reason: "missing"})
	}
	return out
}

func planned(t *testing.T, f *parse.File, st plan.State) []plan.Step {
	t.Helper()
	s, err := plan.Make(f, nil, st)
	if err != nil {
		t.Fatalf("Make: %v", err)
	}
	return s
}

const abcd = "a:\nb:\nc: a\nd:\n"

func TestWavesWithoutLimit(t *testing.T) {
	got := sched.Waves(file(t, abcd), steps("a", "b", "c", "d"), 0)
	if want := [][]string{{"a", "b", "d"}, {"c"}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("Waves = %q, want %q", got, want)
	}
}

func TestWavesWithJobs(t *testing.T) {
	f := file(t, abcd)
	if got, want := sched.Waves(f, steps("a", "b", "c", "d"), 2), [][]string{{"a", "b"}, {"c", "d"}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("jobs 2: Waves = %q, want %q", got, want)
	}
	if got, want := sched.Waves(f, steps("a", "b", "c", "d"), 1), [][]string{{"a"}, {"b"}, {"c"}, {"d"}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("jobs 1: Waves = %q, want %q", got, want)
	}
}

func TestWavesZeroOrNegativeJobsMeansNoLimit(t *testing.T) {
	f := file(t, abcd)
	want := [][]string{{"a", "b", "d"}, {"c"}}
	for _, jobs := range []int{0, -3} {
		if got := sched.Waves(f, steps("a", "b", "c", "d"), jobs); !reflect.DeepEqual(got, want) {
			t.Fatalf("jobs %d: Waves = %q", jobs, got)
		}
	}
}

func TestWavesEmpty(t *testing.T) {
	if got := sched.Waves(file(t, abcd), nil, 0); len(got) != 0 {
		t.Fatalf("Waves(nil) = %q", got)
	}
	if got := sched.Waves(file(t, abcd), []plan.Step{}, 3); len(got) != 0 {
		t.Fatalf("Waves(empty) = %q", got)
	}
}

func TestWavesLaterStepFillsEarlierWave(t *testing.T) {
	f := file(t, "a:\nb:\nc: a\nd: c\ne:\n")
	got := sched.Waves(f, steps("a", "b", "c", "d", "e"), 2)
	if want := [][]string{{"a", "b"}, {"c", "e"}, {"d"}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("Waves = %q, want %q", got, want)
	}
	got = sched.Waves(f, steps("a", "c", "b"), 1)
	if want := [][]string{{"a"}, {"c"}, {"b"}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("Waves = %q, want %q", got, want)
	}
}

func TestWavesOnlyDependenciesThatAreSteps(t *testing.T) {
	f := file(t, "app: lib.o main.o\nlib.o: lib.c\nmain.o: main.c\n")
	st := plan.MapState{"lib.c": 1, "main.c": 9, "lib.o": 5, "main.o": 5, "app": 7}
	got := sched.Waves(f, planned(t, f, st), 0)
	if want := [][]string{{"main.o"}, {"app"}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("Waves = %q, want %q", got, want)
	}
	if got := sched.Waves(f, steps("app"), 0); !reflect.DeepEqual(got, [][]string{{"app"}}) {
		t.Fatalf("Waves = %q", got)
	}
}

func TestWavesDiamond(t *testing.T) {
	f := file(t, "top: l r\nl: base\nr: base x\nbase:\nx:\n")
	got := sched.Waves(f, planned(t, f, plan.MapState{}), 0)
	if want := [][]string{{"base", "x"}, {"l", "r"}, {"top"}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("Waves = %q, want %q", got, want)
	}
}

func TestWavesUsePatternRuleDependencies(t *testing.T) {
	f := file(t, "app: main.o util.o gen.o\n\tld\n%.o: %.c\n\tcc\ngen.c: gen.y\n\tyacc\n")
	got := sched.Waves(f, planned(t, f, plan.MapState{"main.c": 1, "util.c": 1, "gen.y": 1}), 2)
	if want := [][]string{{"main.o", "util.o"}, {"gen.c"}, {"gen.o"}, {"app"}}; !reflect.DeepEqual(got, want) {
		t.Fatalf("Waves = %q, want %q", got, want)
	}
}

func TestCriticalPath(t *testing.T) {
	if got := sched.Critical(file(t, abcd), steps("a", "b", "c", "d")); !reflect.DeepEqual(got, []string{"a", "c"}) {
		t.Fatalf("Critical = %q", got)
	}
	f := file(t, "top: l r\nl: x base\nr: base\nbase:\nx:\n")
	if got := sched.Critical(f, planned(t, f, plan.MapState{})); !reflect.DeepEqual(got, []string{"x", "l", "top"}) {
		t.Fatalf("Critical = %q (first dependency in the wave before)", got)
	}
	g := file(t, "z: y\ny: x\nx:\nlate: q\nq: p\np: o\no:\n")
	if got := sched.Critical(g, steps("x", "y", "z", "o", "p", "q", "late")); !reflect.DeepEqual(got, []string{"o", "p", "q", "late"}) {
		t.Fatalf("Critical = %q", got)
	}
	if got := sched.Critical(g, steps("x", "o")); !reflect.DeepEqual(got, []string{"x"}) {
		t.Fatalf("Critical = %q", got)
	}
	if got := sched.Critical(g, nil); got == nil || len(got) != 0 {
		t.Fatalf("Critical(nil) = %#v", got)
	}
}
