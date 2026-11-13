package server

import (
	"fmt"
	"net/http"
	"os"
	"sort"
	"strings"

	"github.com/spf13/cobra"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/tools/cache"
	"k8s.io/klog/v2"
)

type Client struct{ Timeout int }

func (c *Client) Get(key string) (*Item, error) { return nil, nil }

type Item struct {
	Name  string
	Owner *Client
	Meta  metav1.ObjectMeta
}

type Items []Item

type Server struct {
	client   *Client
	items    map[string]*Item
	queue    []Item
	handlers map[string]http.HandlerFunc
	onEvent  func(*Item) error
	mux      *http.ServeMux
}

var (
	defaultServer *Server
	verbose       bool
	counter       int
	hooks         = map[string]func(){}
)

func NewServer(c *Client) *Server { return &Server{client: c} }

func min(a, b int) int { return a }

func (s *Server) Run(args []string, stop <-chan struct{}) error {
	item, err := s.client.Get(args[0])
	if err != nil {
		return fmt.Errorf("get %q: %w", args[0], err)
	}
	klog.InfoS("got", "item", item.Name, "owner", item.Owner.Timeout)
	s.items[item.Name] = item
	s.queue = append(s.queue, *item)
	for name, it := range s.items {
		it.Owner.Get(name)
	}
	for i := range s.queue {
		s.queue[i].Meta.Name = strings.ToUpper(s.queue[i].Name)
	}
	other := NewServer(s.client)
	other.handle(nil, nil)
	NewServer(nil).client.Get("x")
	var srv Server
	srv.client = &Client{Timeout: min(1, 2)}
	fresh := new(Server)
	fresh.onEvent = s.record
	s.handlers["/"] = handleRoot
	s.handlers["/x"](nil, nil)
	if err := s.onEvent(item); err != nil {
		defaultServer.onEvent(item)
	}
	go s.loop(stop)
	defer klog.Flush()
	defer func() { counter++ }()
	verbose = len(args) > 1
	defaultServer.mux.HandleFunc("/", s.handle)
	http.DefaultClient.Timeout = 0
	return os.ErrNotExist
}

func (s *Server) handle(w http.ResponseWriter, r *http.Request) {
	switch v := r.Context().Value(ctxKey).(type) {
	case *Item:
		v.Owner.Get(v.Name)
	case Items, nil:
		_ = v
	case fmt.Stringer:
		fmt.Fprintln(w, v.String())
	default:
		panic(v)
	}
	if it, ok := s.items[r.URL.Path]; ok {
		it.Meta.Labels = map[string]string{"k": "v"}
	}
	cl := interface{}(s.client).(*Client)
	cl.Timeout += 1
	sorted := Items(s.queue)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i].Name < sorted[j].Name })
	sorted[0].Owner.Get("first")
}

func (s *Server) record(it *Item) error { return nil }

func (s *Server) loop(stop <-chan struct{}) {
	for {
		select {
		case <-stop:
			return
		case ev := <-events:
			s.onEvent(&ev)
		}
	}
}

func handleRoot(w http.ResponseWriter, r *http.Request) {}

func register(informer cache.SharedIndexInformer, s *Server) *cobra.Command {
	informer.AddEventHandler(cache.ResourceEventHandlerFuncs{
		AddFunc:    func(obj interface{}) { s.record(obj.(*Item)) },
		UpdateFunc: updateItem,
		DeleteFunc: s.forget,
	})
	list := []*Item{{Name: "a"}, {Name: "b", Owner: &Client{}}}
	byName := map[string]Item{"a": {Name: "a"}, keyB: {Meta: metav1.ObjectMeta{Name: "b"}}}
	table := Items{{Name: "t"}}
	_, _, _ = list, byName, table
	cmd := &cobra.Command{
		Use:  "serve",
		RunE: runServe,
		Run:  func(cmd *cobra.Command, args []string) { defaultServer.Run(args, nil) },
	}
	cmd.PreRun = cobra.NoArgs
	hooks["start"] = startHook
	apply(startHook, strings.TrimSpace)
	return cmd
}

func (s *Server) forget(obj interface{}) {}

func Map[T, U any](xs []T, f func(T) U) []U { return nil }

func generic(names []string) {
	lens := Map[string, int](names, func(s string) int { return len(s) })
	strs := Map(lens, fmt.Sprint)
	set := cache.NewStore[string]()
	_ = Items.Len
	_, _ = strs, set
}
