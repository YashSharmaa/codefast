// Command crosscheck is the independent verifier's second derivation of the Go
// oracle's static edges. It does not use go/ssa or RTA at all: it walks the
// type-checked syntax (go/ast + go/types) and takes typeutil.StaticCallee for
// every call expression. Its output is compared with the oracle's `static`
// rows by verify/go_crosscheck.py; every disagreement must be explainable.
//
//	crosscheck -repo <module root> [-tests] > ast_edges.jsonl
//
// One JSON object per (call site, callee), same keys as the oracle:
// caller = outermost FuncDecl (or package-level `var f = func(){}`), site =
// position of the called name token, callee = FuncDecl of the generic origin.
// Rows whose caller is package-initialiser code have "caller": null.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/token"
	"go/types"
	"os"
	"path/filepath"
	"strings"

	"golang.org/x/tools/go/packages"
	"golang.org/x/tools/go/types/typeutil"
)

type ep struct {
	File string `json:"file"`
	Name string `json:"name"`
	Line int    `json:"line"`
}
type row struct {
	Caller    *ep    `json:"caller"`
	Callee    ep     `json:"callee"`
	SiteLine  int    `json:"site_line"`
	SiteCol   int    `json:"site_col"`
	Via       string `json:"via,omitempty"`
	InFuncLit bool   `json:"in_func_lit,omitempty"`
	Generic   bool   `json:"caller_generic,omitempty"`
	// dynamic call sites are listed too, so that coverage of sites can be compared
	Dynamic string `json:"dynamic,omitempty"`
}

func main() {
	repo := flag.String("repo", ".", "module root")
	tests := flag.Bool("tests", false, "include tests")
	flag.Parse()
	root, _ := filepath.Abs(*repo)
	root, _ = filepath.EvalSymlinks(root)
	cfg := &packages.Config{
		Mode: packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles | packages.NeedSyntax |
			packages.NeedTypes | packages.NeedTypesInfo | packages.NeedImports | packages.NeedModule,
		Dir: root, Tests: *tests,
	}
	pkgs, err := packages.Load(cfg, "./...")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	rel := func(abs string) string {
		if r, err := filepath.EvalSymlinks(abs); err == nil {
			abs = r
		}
		r, err := filepath.Rel(root, abs)
		if err != nil || strings.HasPrefix(r, "..") {
			return ""
		}
		return filepath.ToSlash(r)
	}
	// name-ident position of every FuncDecl -> its `func` keyword position
	type decl struct {
		file string
		line int
		name string
	}
	declOf := map[token.Pos]decl{}
	enc := json.NewEncoder(os.Stdout)
	enc.SetEscapeHTML(false)
	seenFile := map[string]bool{}
	nerr := 0
	for _, p := range pkgs {
		nerr += len(p.Errors)
		for _, f := range p.Syntax {
			for _, d := range f.Decls {
				if fd, ok := d.(*ast.FuncDecl); ok {
					pp := p.Fset.Position(fd.Pos())
					declOf[fd.Name.Pos()] = decl{rel(pp.Filename), pp.Line, fd.Name.Name}
				}
			}
		}
	}
	for _, p := range pkgs {
		for i, f := range p.Syntax {
			if i >= len(p.CompiledGoFiles) {
				continue
			}
			fr := rel(p.CompiledGoFiles[i])
			if fr == "" || seenFile[fr] || ast.IsGenerated(f) {
				continue
			}
			seenFile[fr] = true
			info, fset := p.TypesInfo, p.Fset
			var walk func(n ast.Node, caller *ep, inLit, generic bool)
			handle := func(c *ast.CallExpr, caller *ep, inLit, generic bool, via string) {
				nt := nameTok(c.Fun, c.Lparen)
				sp := fset.Position(nt)
				r := row{Caller: caller, SiteLine: sp.Line, SiteCol: sp.Column, Via: via, InFuncLit: inLit, Generic: generic}
				if fn := typeutil.StaticCallee(info, c); fn != nil {
					fn = fn.Origin()
					d, ok := declOf[fn.Pos()]
					if !ok || d.file == "" {
						return // external, or a declaration without a body in the repo
					}
					r.Callee = ep{d.file, d.name, d.line}
					enc.Encode(r)
					return
				}
				// not static: classify, to compare dynamic-site coverage
				if tv, ok := info.Types[c.Fun]; ok && tv.IsType() {
					return // conversion
				}
				switch obj := typeutil.Callee(info, c).(type) {
				case *types.Builtin:
					return
				case *types.Func:
					r.Dynamic = "interface"
					_ = obj
				default:
					r.Dynamic = "funcvalue"
				}
				enc.Encode(r)
			}
			walk = func(n ast.Node, caller *ep, inLit, generic bool) {
				ast.Inspect(n, func(m ast.Node) bool {
					switch m := m.(type) {
					case *ast.FuncLit:
						if m != n {
							walk(m, caller, true, generic)
							return false
						}
					case *ast.GoStmt:
						handle(m.Call, caller, inLit, generic, "go")
						for _, a := range m.Call.Args {
							walk(a, caller, inLit, generic)
						}
						walk(m.Call.Fun, caller, inLit, generic)
						return false
					case *ast.DeferStmt:
						handle(m.Call, caller, inLit, generic, "defer")
						for _, a := range m.Call.Args {
							walk(a, caller, inLit, generic)
						}
						walk(m.Call.Fun, caller, inLit, generic)
						return false
					case *ast.CallExpr:
						handle(m, caller, inLit, generic, "")
					}
					return true
				})
			}
			for _, d := range f.Decls {
				switch d := d.(type) {
				case *ast.FuncDecl:
					if d.Body == nil {
						continue
					}
					pp := fset.Position(d.Pos())
					generic := d.Type.TypeParams != nil && len(d.Type.TypeParams.List) > 0
					if d.Recv != nil && len(d.Recv.List) > 0 && strings.Contains(types.ExprString(d.Recv.List[0].Type), "[") {
						generic = true
					}
					walk(d.Body, &ep{fr, d.Name.Name, pp.Line}, false, generic)
				case *ast.GenDecl:
					for _, s := range d.Specs {
						vs, ok := s.(*ast.ValueSpec)
						if !ok {
							continue
						}
						for i, v := range vs.Values {
							if lit, ok := ast.Unparen(v).(*ast.FuncLit); ok && d.Tok == token.VAR && len(vs.Values) == len(vs.Names) && vs.Names[i].Name != "_" {
								pp := fset.Position(vs.Names[i].Pos())
								walk(lit.Body, &ep{fr, vs.Names[i].Name, pp.Line}, false, false)
							} else {
								walk(v, nil, false, false)
							}
						}
					}
				}
			}
		}
	}
	fmt.Fprintf(os.Stderr, "packages=%d files=%d load_errors=%d\n", len(pkgs), len(seenFile), nerr)
}

func nameTok(e ast.Expr, fallback token.Pos) token.Pos {
	switch f := ast.Unparen(e).(type) {
	case *ast.Ident:
		return f.Pos()
	case *ast.SelectorExpr:
		return f.Sel.Pos()
	case *ast.IndexExpr:
		return nameTok(f.X, fallback)
	case *ast.IndexListExpr:
		return nameTok(f.X, fallback)
	case *ast.FuncLit:
		return f.Pos()
	}
	return fallback
}
