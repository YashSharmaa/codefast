package broken

import "fmt"

type Good struct {
	Name string
}

func (g *Good) Hello() string { return fmt.Sprintf("hi %s", g.Name) }

// A statement the parser cannot finish.
func half(g *Good) {
	x := g.Hello(
	fmt.Println(x)
}

type Bad struct {
	Field int
	Other string =
}

func after(g *Good) int {
	if g == nil {
		return 0
	}
	for i := 0; i < 3; i++ {
		fmt.Println(i, g.Name)
	}}
	return len(g.Name)
}

func @@ weird(a int) {}

var tail = Good{Name: "t"}

func last() { tail.Hello() }
