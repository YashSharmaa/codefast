//go:build linux

package edge

/*
#include <stdlib.h>
*/
import "C"

import (
	"errors"
	"unsafe"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/utils/ptr"
)

type Options struct {
	Port     *int32
	Protocol *corev1.Protocol
	Name     *string
	Limits   map[string]int
	Nested   []Options
}

type Matrix [][]float64

type Registry map[Kind]func() Handler

type Handler interface{ Handle(req *Request) error }

type HandlerFunc func(req *Request) error

func (f HandlerFunc) Handle(req *Request) error { return f(req) }

type Request struct {
	Body unsafe.Pointer
	Kind Kind
}

type Kind uint8

const (
	KindA Kind = 1 << iota
	KindB
	_
	KindD = "d"
)

var ErrClosed = errors.New("closed")

var registry = Registry{KindA: newA, KindB: func() Handler { return nil }}

var defaults = Options{
	Port:     new(int32(80)),
	Protocol: new(corev1.ProtocolTCP),
	Name:     new("web-server (default)"),
	Limits:   map[string]int{"cpu": 2},
	Nested:   []Options{{Port: ptr.To[int32](443)}, {Name: new(defaultName())}},
}

// go1.26: new takes an expression.
func pointers(o Options, k Kind) {
	a := new(o.Limits["cpu"] + 1)
	b := new(Options{Name: o.Name})
	c := new(Kind)
	d := new(k)
	e := new(corev1.Pod)
	f := new(-1)
	b.Nested[0].Port = a
	_, _, _, _ = c, d, e, f
}

func newA() Handler { return HandlerFunc(serve) }

func serve(req *Request) error {
	p := (*C.char)(req.Body)
	C.free(unsafe.Pointer(p))
	size := unsafe.Sizeof(*req)
	m := Matrix{{1, 2}, {3, float64(size)}}
	h := registry[req.Kind]()
	if err := h.Handle(req); errors.Is(err, ErrClosed) {
		return (&Request{Kind: KindB}).validate(m[0][1:]...)
	}
	fn := HandlerFunc.Handle
	bound := h.Handle
	var iface interface {
		Handler
		Close() error
	}
	anon := struct {
		Handler
		n int
	}{Handler: h}
	_ = iface.Close()
	return errors.Join(fn(nil, req), bound(req), anon.Handle(req), <-make(chan error))
}

func (r *Request) validate(row ...float64) (err error) {
	defer func() {
		if e, ok := recover().(error); ok {
			err = e
		}
	}()
	type local = Request
	var cp local = *r
	cp.Kind++
	lookup := func(k Kind) (h Handler, found bool) {
		f, found := registry[k]
		return f(), found
	}
	h, _ := lookup(cp.Kind)
	return h.Handle(&cp)
}

func compose[T any, PT interface{ *T; Handler }](fs ...func(PT) error) func(PT) error {
	return func(p PT) error {
		for _, f := range fs {
			if err := f(p); err != nil {
				return err
			}
		}
		return p.Handle(nil)
	}
}
