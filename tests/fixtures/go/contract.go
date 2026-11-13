// Package contract exercises the 2026-11-13 contract rules: module-qualified names, receiver
// type paths, and which import path a package name is bound to.
package contract

import (
	"encoding/json"
	"io"
	"testing"

	// An explicit alias wins over the default name of another import, whatever the order.
	"example.com/app/internal/ui"
	handler "example.com/app/cli/ui"
	client "example.com/sdk/transport"
	"example.com/legacy/client"

	// The last path element wins over the element before a version.
	"example.com/apis/autoscaling"
	"example.com/apis/autoscaling/v2"
	"k8s.io/klog/v2"

	// Two imports with one default name: a package of unknown path.
	"example.com/a/util"
	"example.com/b/util"
	exectesting "k8s.io/utils/exec/testing"

	"github.com/scylladb/go-set/strset"
	"gopkg.in/yaml.v3"
)

type Config struct {
	Tags    *strset.Set
	Out     io.Writer
	Scaler  autoscaling.Scaler
	Targets []v2.MetricTarget
	strset.Options
	*json.Decoder
	handler.Base[string]
	local
}

type local struct{ cfg *Config }

type Alias = handler.Config

type Renderer handler.Renderer

var Default handler.Config

var decoder = json.NewDecoder(nil)

var _ ui.Model = (*Config)(nil)

func Load(r io.Reader) (*handler.Config, error) {
	dec := json.NewDecoder(r)
	var v yaml.Node
	dec.Decode(&v)
	ui.Default.Load()
	ui.None(r).Render()
	handler.New(handler.DefaultHandlerConfig()).Handle(nil)
	client.Dial("x").Close()
	autoscaling.Scale(1)
	v2.NewMetric().Observe(2)
	klog.Infof("x")
	util.Join("a", "b")
	var t testing.T
	exectesting.FakeExec{}.Run(&t)
	return &handler.Config{Name: "x"}, nil
}

func (c *Config) Apply(in <-chan handler.Event, quit chan struct{}, byID map[handler.ID]handler.Event) *Config {
	for ev := range in {
		ev.Ack()
	}
	for i := range c.Targets {
		i.Value()
	}
	for id := range byID {
		id.String()
	}
	for _, ev := range byID {
		ev.Ack()
	}
	out := make(chan *Config)
	for next := range out {
		next.Tags.Add("x")
	}
	var pending chan handler.Event
	for ev := range (pending) {
		ev.Nack()
	}
	first, ok := <-in
	first.Ack()
	_ = ok
	c.Tags.Add("y")
	c.Decode(nil)
	c.Options.Reset()
	c.local.cfg.Scaler.Scale(3)
	return c
}

func (c *Config) With(k string) *Config { return c }

func chain() {
	Default.Renderer.Render()
	decoder.Decode(nil)
	NewConfig().With("a").With("b").With("c").With("d").With("e").With("f").With("g").With("h").With("i").With("j").With("k").With("l").Apply(nil, nil, nil)
	new(Config).With("z").Tags.Clear()
	(&Config{}).Apply(nil, nil, nil).Out.Write(nil)
}

func NewConfig() *Config { return &Config{} }

// Method expressions: the type is the qualifier, the receiver is not an argument.
func methodExprs(c *Config, other *Remote) {
	apply := (*Config).With
	apply(c, "k")
	(*Config).With(c, "k")
	Config.String(*c)
	(*Remote).Dial(other, "addr", 1)
	(*handler.Config).Validate(nil)
	walk(c, (*Remote).Close, Remote.Name)
	(*Default).Renderer.Render()
	(*other).addr = "x"
}

// A scope gives a name back to the scope around it, type and channel-ness included.
func scopes(in <-chan handler.Event) {
	x := NewConfig()
	{
		x := in
		for v := range x {
			v.Ack()
		}
	}
	x, err := Load(nil)
	x.With("a")
	for v := range x.Targets {
		v.Value()
	}
	if in := x.Targets; in != nil {
		for v := range in {
			v.Value()
		}
	}
	for v := range in {
		v.Nack()
	}
	_ = err
}

// A struct written inside a signature has no field definitions.
type Visit func(at struct{ Depth int }) (res struct{ Done bool })

var visit func(at struct{ Depth int }) *Config
