// Package shapes exercises every kind of Go definition.
package shapes

import (
	"context"
	"fmt"
	"io"
	. "math"
	_ "net/http/pprof"
	"sync"

	restful "github.com/emicklei/go-restful/v3"
	jsonpatch "gopkg.in/evanphx/json-patch.v4"
	"gopkg.in/yaml.v3"
	v1 "k8s.io/api/core/v1"
	"k8s.io/klog/v2"
	"k8s.io/mount-utils"
)

// Kind is an enumeration.
type Kind int

const (
	KindCircle Kind = iota
	KindSquare
	kindHidden
)

const MaxSides, minSides = 8, 3

const Pi2 float64 = Pi * 2

var (
	Default  = &Circle{Radius: 1}
	registry map[string]Shape
	mu       sync.Mutex
	onChange func(old, cur Shape) error
	logf     = func(format string, args ...any) { klog.Infof(format, args...) }
)

var a, b int = 1, 2

var options struct {
	Verbose bool
	Out     io.Writer
}

// Shape is implemented by every figure.
type Shape interface {
	fmt.Stringer
	Namer
	Area() float64
	Scale(factor float64, more ...float64) (Shape, error)
	visit(ctx context.Context, fn func(Shape) bool)
}

type Namer interface{ Name() string }

type Number interface {
	~int | ~int64 | ~float64
}

// Circle is a round shape.
type Circle struct {
	Base
	*sync.RWMutex
	v1.ObjectMeta
	Radius   float64 `json:"radius"`
	X, Y     float64
	Tags     map[string]*Tag
	OnResize func(c *Circle, by float64) error `json:"-"`
	parent   *Circle
	Style    struct {
		Color string
		Width int
	}
	_ [4]byte
}

type Base struct{ ID Kind }

type Tag struct{ Key, Value string }

type (
	Shapes   []Shape
	ByName   map[string]*Circle
	Visitor  func(ctx context.Context, s Shape) (*Tag, error)
	Pod      = v1.Pod
	Stream   <-chan *Tag
	TagPtr   *Tag
	Document yaml.Node
)

// Stack is a generic container.
type Stack[T any] struct {
	items []T
	limit int
}

type Pair[K comparable, V Number] struct {
	Key K
	Val V
}

type List[T any] = []T

var _ Shape = (*Circle)(nil)
var _ fmt.Stringer = Kind(0)
var _ io.Closer = &Stack[int]{}

func (k Kind) String() string { return fmt.Sprint(int(k)) }

func (c *Circle) Area() float64 { return Pi * c.Radius * c.Radius }

func (c *Circle) Scale(factor float64, more ...float64) (Shape, error) {
	return &Circle{Radius: c.Radius * factor}, nil
}

func (*Circle) Name() string { return "circle" }

func (c Circle) visit(ctx context.Context, fn func(Shape) bool) {}

func (s *Stack[T]) Push(v T, more ...T) (ok bool) {
	s.items = append(s.items, v)
	return true
}

func (s *Stack[T]) Close() error { return nil }

func (p Pair[K, V]) Swap() (V, K) { return p.Val, p.Key }

// NewStack builds a stack.
func NewStack[T any, U comparable](limit int) *Stack[T] { return &Stack[T]{limit: limit} }

func Describe(ctx context.Context, shapes ...Shape) (names []string, err error) { return }

func Mount(m mount.Interface, ws *restful.WebService, p jsonpatch.Patch) (io.Reader, *v1.Pod) {
	return nil, nil
}

func helper(a, b int, _ string) *Tag

func init() { registry = map[string]Shape{} }
