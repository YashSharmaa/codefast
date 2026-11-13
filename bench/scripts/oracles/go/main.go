// Command go-oracle builds a compiler-grade call-graph answer key for a Go
// module, in the codefast benchmark oracle format (bench/oracles/FORMAT.md).
//
// Two sources of truth, both from golang.org/x/tools, neither a tool under test:
//
//   - static edges: every call instruction in the type-checked SSA program whose
//     callee the compiler resolved statically (ssa.CallCommon.StaticCallee).
//     These are emitted for every function declared in the module, whether or
//     not anything reaches it.
//   - dynamic edges: interface-method invocations and calls through function
//     values, resolved by Rapid Type Analysis (go/callgraph/rta) rooted at every
//     main.main (and, with -tests, every synthesised test main). One call site
//     yields one edge per possible target (its candidate set).
//
// The methodology follows the published repowise-bench G4 experiment (same
// loader mode, same SSA mode, same RTA roots, caller keyed at the outermost
// enclosing declared function, edges keyed on declaration positions). The code
// is an independent implementation and additionally keeps static edges of
// functions RTA never reached, the call-site position, the dispatch kind, and a
// functions listing.
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
	"runtime"
	"runtime/debug"
	"sort"
	"strings"

	"golang.org/x/tools/go/callgraph"
	"golang.org/x/tools/go/callgraph/rta"
	"golang.org/x/tools/go/packages"
	"golang.org/x/tools/go/ssa"
	"golang.org/x/tools/go/ssa/ssautil"
)

type endpoint struct {
	File     string `json:"file"`
	Name     string `json:"name"`
	Line     int    `json:"line"`
	Receiver string `json:"receiver,omitempty"`
}

type closureRef struct {
	Name string `json:"name"`
	Line int    `json:"line"`
}

type edge struct {
	Caller   endpoint `json:"caller"`
	Callee   endpoint `json:"callee"`
	SiteLine int      `json:"site_line"`
	SiteCol  int      `json:"site_col"`
	Kind     string   `json:"kind"`
	Via      string   `json:"via,omitempty"`
	// Number of distinct in-repo targets the analysis found for this call site
	// (dynamic kinds only), and how many more lie outside the repository.
	Candidates         int `json:"candidates,omitempty"`
	CandidatesExternal int `json:"candidates_external,omitempty"`
	// "generic-instantiation": a call on a type parameter (`x.M()` with x of
	// type T), statically resolved by the compiler in each instantiation of
	// the enclosing generic function. One row per instantiation target.
	CandidateReason string `json:"candidate_reason,omitempty"`
	// Set when the call is written inside a func literal; the caller above is
	// still the outermost enclosing declared function.
	CallerClosure *closureRef `json:"caller_closure,omitempty"`
	// Set when the target is a func literal. callee is then the literal's own
	// position and callee_outer the declared function it is written in.
	CalleeClosure bool      `json:"callee_closure,omitempty"`
	CalleeOuter   *endpoint `json:"callee_outer,omitempty"`
	RTAReachable  bool      `json:"rta_reachable"`
	Implicit      bool      `json:"implicit,omitempty"`
}

type funcRec struct {
	File     string `json:"file"`
	Name     string `json:"name"`
	Line     int    `json:"line"`
	EndLine  int    `json:"end_line"`
	Receiver string `json:"receiver_or_class"`
	Qualname string `json:"qualname"`
	Kind     string `json:"kind"`
	Exported bool   `json:"exported"`
	Generic  bool   `json:"generic,omitempty"`
	// judged: the oracle asserts it knows every in-repo call edge out of this
	// function, so an edge a tool reports from it that the oracle lacks is
	// contradicted rather than unjudged. See FORMAT.md.
	Judged bool `json:"judged"`
	// Reasons other than an unresolved dynamic site (those are counted below
	// and listed in unresolved_sites.jsonl) for judged being false.
	UnjudgedReasons []string `json:"unjudged_reasons,omitempty"`
	RTAReachable    bool     `json:"rta_reachable"`
	DynamicSites    int      `json:"dynamic_sites"`
	UnresolvedSites int      `json:"unresolved_dynamic_sites"`
	Closures        int      `json:"closures"`
}

type unresolvedSite struct {
	Caller   endpoint `json:"caller"`
	SiteLine int      `json:"site_line"`
	SiteCol  int      `json:"site_col"`
	Kind     string   `json:"kind"`
	Reason   string   `json:"reason"`
}

type declKey struct {
	file string
	line int
}

type declInfo struct {
	rec     *funcRec
	nameTok token.Pos
}

func main() {
	repo := flag.String("repo", ".", "module root to analyse")
	outDir := flag.String("out", "", "output directory (edges.jsonl, functions.jsonl, meta.json)")
	withTests := flag.Bool("tests", false, "include _test.go files and test mains")
	suffix := flag.String("suffix", "", "suffix inserted before .jsonl/.json in output names, e.g. .with_tests")
	repoName := flag.String("name", "", "repository name recorded in meta.json")
	commit := flag.String("commit", "", "commit recorded in meta.json")
	flag.Parse()
	if *outDir == "" {
		die(fmt.Errorf("-out is required"))
	}
	root, err := filepath.Abs(*repo)
	if err != nil {
		die(err)
	}
	root, _ = filepath.EvalSymlinks(root)

	cfg := &packages.Config{
		Mode: packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles |
			packages.NeedSyntax | packages.NeedTypes | packages.NeedTypesInfo |
			packages.NeedDeps | packages.NeedImports | packages.NeedModule,
		Dir:   root,
		Tests: *withTests,
	}
	pkgs, err := packages.Load(cfg, "./...")
	if err != nil {
		die(err)
	}
	var loadErrors []string
	packages.Visit(pkgs, nil, func(p *packages.Package) {
		for _, e := range p.Errors {
			loadErrors = append(loadErrors, e.Error())
		}
	})
	for i, e := range loadErrors {
		if i < 20 {
			fmt.Fprintln(os.Stderr, "load error:", e)
		}
	}

	relOf := func(abs string) string {
		if abs == "" {
			return ""
		}
		if r, err := filepath.EvalSymlinks(abs); err == nil {
			abs = r
		}
		rel, err := filepath.Rel(root, abs)
		if err != nil || strings.HasPrefix(rel, "..") {
			return ""
		}
		return filepath.ToSlash(rel)
	}

	// ---- file scope ------------------------------------------------------
	analysed := map[string]bool{}
	excluded := map[string]string{} // rel -> reason
	ignored := map[string]bool{}
	var fset *token.FileSet
	type fileSyn struct {
		rel string
		f   *ast.File
		pkg *packages.Package
	}
	var syn []fileSyn
	seenFile := map[string]bool{}
	packages.Visit(pkgs, nil, func(p *packages.Package) {
		if fset == nil && p.Fset != nil {
			fset = p.Fset
		}
		for _, gf := range p.IgnoredFiles {
			if r := relOf(gf); r != "" {
				ignored[r] = true
			}
		}
		for i, gf := range p.CompiledGoFiles {
			r := relOf(gf)
			if r == "" || !strings.HasSuffix(r, ".go") {
				continue
			}
			if i >= len(p.Syntax) {
				continue
			}
			f := p.Syntax[i]
			if ast.IsGenerated(f) {
				excluded[r] = "generated"
				continue
			}
			analysed[r] = true
			if !seenFile[r] {
				seenFile[r] = true
				syn = append(syn, fileSyn{r, f, p})
			}
		}
	})
	for r := range analysed {
		delete(ignored, r)
	}
	if fset == nil {
		die(fmt.Errorf("no packages loaded"))
	}

	posOf := func(p token.Pos) (string, int, int) {
		if !p.IsValid() {
			return "", 0, 0
		}
		pp := fset.Position(p)
		return relOf(pp.Filename), pp.Line, pp.Column
	}

	// ---- declarations from syntax -----------------------------------------
	decls := map[declKey]*declInfo{}       // keyed by position of the `func` keyword (FuncDecl) or var name (var_func)
	byNamePos := map[token.Pos]*declInfo{} // FuncDecl name ident pos, and FuncLit pos for var_func
	callByLparen := map[token.Pos]*ast.CallExpr{}
	for _, fsyn := range syn {
		pkgPath := fsyn.pkg.PkgPath
		for _, d := range fsyn.f.Decls {
			switch d := d.(type) {
			case *ast.FuncDecl:
				if d.Body == nil {
					continue // assembly / linkname stub: a declaration, not a definition
				}
				_, line, _ := posOf(d.Pos())
				_, endLine, _ := posOf(d.End())
				rec := &funcRec{File: fsyn.rel, Name: d.Name.Name, Line: line, EndLine: endLine,
					Kind: "function", Exported: d.Name.IsExported()}
				if d.Recv != nil && len(d.Recv.List) > 0 {
					rec.Kind = "method"
					rec.Receiver = types.ExprString(d.Recv.List[0].Type)
					rec.Qualname = pkgPath + ".(" + rec.Receiver + ")." + d.Name.Name
					if strings.Contains(rec.Receiver, "[") {
						rec.Generic = true
					}
				} else {
					rec.Qualname = pkgPath + "." + d.Name.Name
				}
				if d.Type.TypeParams != nil && len(d.Type.TypeParams.List) > 0 {
					rec.Generic = true
				}
				di := &declInfo{rec: rec, nameTok: d.Name.Pos()}
				decls[declKey{fsyn.rel, line}] = di
				byNamePos[d.Name.Pos()] = di
				byNamePos[d.Pos()] = di
			case *ast.GenDecl:
				if d.Tok != token.VAR {
					continue
				}
				for _, spec := range d.Specs {
					vs, ok := spec.(*ast.ValueSpec)
					if !ok || len(vs.Values) != len(vs.Names) {
						continue
					}
					for i, v := range vs.Values {
						lit, ok := ast.Unparen(v).(*ast.FuncLit)
						if !ok || vs.Names[i].Name == "_" {
							continue
						}
						_, line, _ := posOf(vs.Names[i].Pos())
						_, endLine, _ := posOf(lit.End())
						rec := &funcRec{File: fsyn.rel, Name: vs.Names[i].Name, Line: line, EndLine: endLine,
							Kind: "var_func", Exported: vs.Names[i].IsExported(),
							Qualname: pkgPath + "." + vs.Names[i].Name}
						di := &declInfo{rec: rec, nameTok: vs.Names[i].Pos()}
						decls[declKey{fsyn.rel, line}] = di
						byNamePos[lit.Pos()] = di
					}
				}
			}
		}
		ast.Inspect(fsyn.f, func(n ast.Node) bool {
			if c, ok := n.(*ast.CallExpr); ok {
				callByLparen[c.Lparen] = c
			}
			return true
		})
	}

	// ---- SSA + RTA ---------------------------------------------------------
	prog, _ := ssautil.AllPackages(pkgs, ssa.InstantiateGenerics)
	prog.Build()

	var roots []*ssa.Function
	var mains []string
	for _, p := range prog.AllPackages() {
		if p.Pkg.Name() != "main" {
			continue
		}
		if fn := p.Func("main"); fn != nil {
			roots = append(roots, fn)
			mains = append(mains, p.Pkg.Path())
		}
		if fn := p.Func("init"); fn != nil {
			roots = append(roots, fn)
		}
	}
	sort.Strings(mains)
	var cg *callgraph.Graph
	reached := map[*ssa.Function]bool{}
	if len(roots) > 0 {
		res := rta.Analyze(roots, true)
		cg = res.CallGraph
		for fn := range cg.Nodes {
			if fn != nil {
				reached[fn] = true
			}
		}
	} else {
		fmt.Fprintln(os.Stderr, "warning: no main package; RTA has no roots, dynamic edges will be empty")
	}

	outermost := func(fn *ssa.Function) *ssa.Function {
		for fn.Parent() != nil {
			fn = fn.Parent()
		}
		return fn
	}
	// declOf maps an SSA function to the declared function it is written in.
	// A func literal that directly initialises a package-level var is itself a
	// declaration (kind var_func). Returns nil for synthetic functions and for
	// code at package-initialiser level.
	declOf := func(fn *ssa.Function) *declInfo {
		for f := fn; f != nil; f = f.Parent() {
			if lit, ok := f.Syntax().(*ast.FuncLit); ok {
				if di := byNamePos[lit.Pos()]; di != nil {
					return di
				}
			}
		}
		top := outermost(fn)
		if d, ok := top.Syntax().(*ast.FuncDecl); ok {
			return byNamePos[d.Name.Pos()]
		}
		return nil
	}
	isGenericBody := func(fn *ssa.Function) bool {
		for f := fn; f != nil; f = f.Parent() {
			if f.TypeParams().Len() > 0 && len(f.TypeArgs()) == 0 {
				return true
			}
		}
		return false
	}
	// isClosure reports whether fn is a nested function that has no declaration
	// of its own: a func literal (other than a var_func declaration) or the
	// compiler-synthesised body of a range-over-func loop.
	isClosure := func(fn *ssa.Function) bool {
		if fn.Parent() == nil {
			return false
		}
		if lit, ok := fn.Syntax().(*ast.FuncLit); ok && byNamePos[lit.Pos()] != nil {
			return false
		}
		return true
	}
	closureName := func(fn *ssa.Function) string { return fn.Name() }

	type target struct {
		ep       endpoint
		closure  bool
		outer    *endpoint
		external bool
	}
	epOf := func(di *declInfo) endpoint {
		return endpoint{File: di.rec.File, Name: di.rec.Name, Line: di.rec.Line, Receiver: di.rec.Receiver}
	}
	// resolve maps a callee SSA function to source-level targets, looking
	// through compiler-synthesised wrappers: static calls inside any wrapper
	// (promoted methods of embedded structs, instantiation wrappers), and the
	// interface invoke inside a bound-method closure or thunk.
	var resolve func(fn *ssa.Function, seen map[*ssa.Function]bool) []target
	resolve = func(fn *ssa.Function, seen map[*ssa.Function]bool) []target {
		if fn == nil || seen[fn] {
			return nil
		}
		seen[fn] = true
		if fn.Syntax() != nil {
			if isClosure(fn) {
				f, l, _ := posOf(fn.Pos())
				if f == "" || !analysed[f] {
					return []target{{external: true}}
				}
				t := target{ep: endpoint{File: f, Name: closureName(fn), Line: l}, closure: true}
				if di := declOf(fn); di != nil {
					o := epOf(di)
					t.outer = &o
				}
				return []target{t}
			}
			if di := declOf(fn); di != nil {
				return []target{{ep: epOf(di)}}
			}
			return []target{{external: true}}
		}
		if fn.Blocks == nil {
			return []target{{external: true}} // no body: external or assembly
		}
		// Synthetic with a body: follow its calls.
		var out []target
		for _, b := range fn.Blocks {
			for _, ins := range b.Instrs {
				site, ok := ins.(ssa.CallInstruction)
				if !ok {
					continue
				}
				if c := site.Common().StaticCallee(); c != nil {
					out = append(out, resolve(c, seen)...)
				} else if cg != nil && (strings.HasSuffix(fn.Name(), "$bound") || strings.HasSuffix(fn.Name(), "$thunk")) {
					// A method value or method expression on an interface
					// (`f := x.M`): the wrapper's invoke is the user's call.
					// A promoted-method wrapper that dispatches through an
					// embedded interface is NOT followed: that is a second
					// hop through a type, not this call site's target.
					if n := cg.Nodes[fn]; n != nil {
						for _, e := range n.Out {
							if e.Site == site {
								out = append(out, resolve(e.Callee.Func, seen)...)
							}
						}
					}
				}
			}
		}
		return out
	}

	type siteKey struct {
		di        *declInfo
		line, col int
	}
	type siteInfo struct {
		kind     string
		covered  bool // some non-generic-origin SSA body containing it was reached by RTA
		concrete bool // seen in a non-generic-origin body
		pos      token.Pos
	}
	edges := map[string]*edge{}
	siteTargets := map[siteKey]map[endpoint]bool{}
	siteExternal := map[siteKey]map[string]bool{}
	dynSites := map[siteKey]*siteInfo{}
	staticSites := map[siteKey]bool{} // sites with at least one in-repo static row
	closuresOf := map[*declInfo]map[token.Pos]bool{}
	reachedDecl := map[*declInfo]bool{}
	positionless := map[*declInfo]bool{} // has an implicit call, without a source position, to a declared function
	// An instantiation wrapper carries the syntax of the generic function it
	// forwards to, but its body is one compiler-made forwarding call: it is
	// not a caller.
	isWrapper := func(fn *ssa.Function) bool {
		return strings.HasPrefix(fn.Synthetic, "instantiation wrapper")
	}
	counts := map[string]int{}

	sitePos := func(site ssa.CallInstruction) (token.Pos, bool) {
		// Common().Pos() is the call's left parenthesis; for go/defer the
		// instruction's own Pos() is the keyword instead.
		p := site.Common().Pos()
		if !p.IsValid() {
			p = site.Pos()
		}
		if c := callByLparen[p]; c != nil {
			return nameTok(c.Fun, c.Lparen), false
		}
		return p, true
	}
	via := func(site ssa.CallInstruction) string {
		switch site.(type) {
		case *ssa.Go:
			return "go"
		case *ssa.Defer:
			return "defer"
		}
		return ""
	}
	dynKind := func(c *ssa.CallCommon) string {
		if c.IsInvoke() {
			return "interface"
		}
		return "funcptr"
	}

	emit := func(fn *ssa.Function, site ssa.CallInstruction, callee *ssa.Function, kind string) {
		di := declOf(fn)
		if di == nil {
			counts["calls_at_package_init_level"]++
			return
		}
		p, implicit := sitePos(site)
		sf, sl, sc := posOf(p)
		if sf == "" || !analysed[sf] {
			// The implicit call of the iterator in a range-over-func loop has
			// no position. It cannot be written as a row; if it reaches a
			// declared function the caller is no longer fully known.
			counts["site_without_position"]++
			for _, t := range resolve(callee, map[*ssa.Function]bool{}) {
				if !t.external && !t.closure {
					counts["site_without_position_to_declared_function"]++
					positionless[di] = true
				}
			}
			if os.Getenv("ORACLE_DEBUG") != "" {
				fmt.Fprintf(os.Stderr, "no-pos site: caller=%s callee=%s kind=%s instr=%s\n", fn, callee, kind, site)
			}
			return
		}
		sk := siteKey{di, sl, sc}
		for _, t := range resolve(callee, map[*ssa.Function]bool{}) {
			if t.external {
				if kind != "static" {
					if siteExternal[sk] == nil {
						siteExternal[sk] = map[string]bool{}
					}
					siteExternal[sk][callee.String()] = true
				} else {
					counts["static_calls_to_external"]++
				}
				continue
			}
			e := &edge{Caller: epOf(di), Callee: t.ep, SiteLine: sl, SiteCol: sc, Kind: kind,
				Via: via(site), CalleeClosure: t.closure, CalleeOuter: t.outer,
				RTAReachable: reached[fn], Implicit: implicit}
			if isClosure(fn) {
				_, cl, _ := posOf(fn.Pos())
				e.CallerClosure = &closureRef{Name: closureName(fn), Line: cl}
			}
			k := fmt.Sprintf("%s\x00%d\x00%d\x00%d\x00%s\x00%d\x00%s", e.Caller.File, e.Caller.Line, sl, sc, e.Callee.File, e.Callee.Line, kind)
			if old := edges[k]; old != nil {
				old.RTAReachable = old.RTAReachable || e.RTAReachable
			} else {
				edges[k] = e
			}
			if kind != "static" {
				if siteTargets[sk] == nil {
					siteTargets[sk] = map[endpoint]bool{}
				}
				siteTargets[sk][t.ep] = true
			} else {
				staticSites[sk] = true
			}
		}
	}

	allFns := ssautil.AllFunctions(prog)
	// ssautil.AllFunctions does not visit the generic origin of a method of a
	// generic type (Program.MethodValue is nil for it) unless something it
	// did visit refers to it, so the calls in such a body would be lost while
	// the function is still listed and judged. Walk the syntax instead: every
	// FuncDecl with a body must have its SSA function in the set.
	var addFn func(fn *ssa.Function)
	addFn = func(fn *ssa.Function) {
		if fn == nil || allFns[fn] {
			return
		}
		allFns[fn] = true
		for _, a := range fn.AnonFuncs {
			addFn(a)
		}
	}
	for _, fsyn := range syn {
		for _, d := range fsyn.f.Decls {
			fd, ok := d.(*ast.FuncDecl)
			if !ok || fd.Body == nil || fd.Name.Name == "_" {
				continue
			}
			obj, _ := fsyn.pkg.TypesInfo.Defs[fd.Name].(*types.Func)
			var fn *ssa.Function
			if obj != nil {
				fn = prog.FuncValue(obj)
			}
			switch {
			case fn == nil || fn.Blocks == nil:
				if fd.Name.Name != "init" { // each init is a numbered synthetic function, reached from the package initialiser
					counts["declared_functions_without_ssa_body"]++
					fmt.Fprintf(os.Stderr, "warning: no SSA body for %s:%s\n", fsyn.rel, fd.Name.Name)
				}
			case !allFns[fn]:
				counts["declared_functions_missed_by_AllFunctions"]++
				addFn(fn)
			}
		}
	}
	for fn := range allFns {
		if fn.Blocks == nil || isWrapper(fn) {
			continue
		}
		di := declOf(fn)
		if di == nil {
			// package initialiser bodies and synthetic wrappers: count the
			// former, skip the latter.
			if fn.Syntax() == nil && fn.Synthetic == "package initializer" {
				if f, _, _ := posOf(fn.Pos()); f != "" || fn.Pkg != nil {
					for _, b := range fn.Blocks {
						for _, ins := range b.Instrs {
							if site, ok := ins.(ssa.CallInstruction); ok {
								if pf, _, _ := posOf(site.Pos()); pf != "" && analysed[pf] {
									counts["calls_at_package_init_level"]++
								}
							}
						}
					}
				}
			}
			continue
		}
		if reached[fn] {
			reachedDecl[di] = true
		}
		if isClosure(fn) {
			if closuresOf[di] == nil {
				closuresOf[di] = map[token.Pos]bool{}
			}
			closuresOf[di][fn.Pos()] = true
		}
		generic := isGenericBody(fn)
		for _, b := range fn.Blocks {
			for _, ins := range b.Instrs {
				site, ok := ins.(ssa.CallInstruction)
				if !ok {
					continue
				}
				c := site.Common()
				if callee := c.StaticCallee(); callee != nil {
					emit(fn, site, callee, "static")
					continue
				}
				if _, isBuiltin := c.Value.(*ssa.Builtin); isBuiltin {
					continue
				}
				p, _ := sitePos(site)
				sf, sl, sc := posOf(p)
				if sf == "" || !analysed[sf] {
					continue
				}
				sk := siteKey{di, sl, sc}
				si := dynSites[sk]
				if si == nil {
					si = &siteInfo{kind: dynKind(c), pos: p}
					dynSites[sk] = si
				}
				if !generic {
					si.concrete = true
					if reached[fn] {
						si.covered = true
					}
				}
			}
		}
	}
	if cg != nil {
		for fn, node := range cg.Nodes {
			if fn == nil || isWrapper(fn) || declOf(fn) == nil {
				continue
			}
			for _, e := range node.Out {
				if e.Site == nil {
					continue
				}
				c := e.Site.Common()
				if c.StaticCallee() != nil {
					continue
				}
				if _, isBuiltin := c.Value.(*ssa.Builtin); isBuiltin {
					continue
				}
				emit(fn, e.Site, e.Callee.Func, dynKind(c))
			}
		}
	}

	// ---- candidate counts, judged flags ------------------------------------
	// A site that is a static call in one SSA body and a dynamic call in
	// another is a call on a type parameter: dynamic in the generic origin,
	// static in each instantiation. Its rows are a candidate set, one per
	// instantiation target, not a single compiler-resolved target.
	for _, e := range edges {
		if e.Kind != "static" {
			continue
		}
		di := decls[declKey{e.Caller.File, e.Caller.Line}]
		sk := siteKey{di, e.SiteLine, e.SiteCol}
		if dynSites[sk] != nil {
			e.CandidateReason = "generic-instantiation"
			if siteTargets[sk] == nil {
				siteTargets[sk] = map[endpoint]bool{}
			}
			siteTargets[sk][e.Callee] = true
		}
	}
	for _, e := range edges {
		if e.Kind == "static" && e.CandidateReason == "" {
			continue
		}
		di := decls[declKey{e.Caller.File, e.Caller.Line}]
		sk := siteKey{di, e.SiteLine, e.SiteCol}
		e.Candidates = len(siteTargets[sk])
		e.CandidatesExternal = len(siteExternal[sk])
		if e.CandidateReason != "" {
			counts["static_rows_generic_instantiation"]++
		}
	}
	var unresolved []unresolvedSite
	for sk, si := range dynSites {
		sk.di.rec.DynamicSites++
		// Dynamic only in a generic origin body, and resolved statically in
		// every instantiation the analysed code makes: not unresolved.
		if !si.concrete && staticSites[sk] {
			continue
		}
		if !si.covered {
			sk.di.rec.UnresolvedSites++
			reason := "caller not reached by RTA"
			if !si.concrete {
				reason = "generic function body never instantiated in a reached function"
			}
			unresolved = append(unresolved, unresolvedSite{Caller: epOf(sk.di), SiteLine: sk.line, SiteCol: sk.col, Kind: si.kind, Reason: reason})
		}
	}
	var funcs []*funcRec
	nJudged := 0
	for _, di := range decls {
		di.rec.RTAReachable = reachedDecl[di]
		di.rec.Closures = len(closuresOf[di])
		di.rec.Judged = di.rec.UnresolvedSites == 0
		if positionless[di] {
			di.rec.Judged = false
			di.rec.UnjudgedReasons = append(di.rec.UnjudgedReasons, "implicit call without a source position (range-over-func)")
		}
		if di.rec.Judged {
			nJudged++
		}
		funcs = append(funcs, di.rec)
	}
	sort.Slice(funcs, func(i, j int) bool {
		if funcs[i].File != funcs[j].File {
			return funcs[i].File < funcs[j].File
		}
		return funcs[i].Line < funcs[j].Line
	})
	var elist []*edge
	for _, e := range edges {
		elist = append(elist, e)
	}
	sort.Slice(elist, func(i, j int) bool {
		a, b := elist[i], elist[j]
		if a.Caller.File != b.Caller.File {
			return a.Caller.File < b.Caller.File
		}
		if a.SiteLine != b.SiteLine {
			return a.SiteLine < b.SiteLine
		}
		if a.SiteCol != b.SiteCol {
			return a.SiteCol < b.SiteCol
		}
		if a.Callee.File != b.Callee.File {
			return a.Callee.File < b.Callee.File
		}
		if a.Callee.Line != b.Callee.Line {
			return a.Callee.Line < b.Callee.Line
		}
		return a.Kind < b.Kind
	})
	sort.Slice(unresolved, func(i, j int) bool {
		a, b := unresolved[i], unresolved[j]
		if a.Caller.File != b.Caller.File {
			return a.Caller.File < b.Caller.File
		}
		if a.SiteLine != b.SiteLine {
			return a.SiteLine < b.SiteLine
		}
		return a.SiteCol < b.SiteCol
	})

	// ---- write -------------------------------------------------------------
	if err := os.MkdirAll(*outDir, 0o755); err != nil {
		die(err)
	}
	kindCount := map[string]int{}
	closureCallee := 0
	for _, e := range elist {
		kindCount[e.Kind]++
		if e.CalleeClosure {
			closureCallee++
		}
	}
	writeJSONL(filepath.Join(*outDir, "edges"+*suffix+".jsonl"), len(elist), func(i int) any { return elist[i] })
	writeJSONL(filepath.Join(*outDir, "functions"+*suffix+".jsonl"), len(funcs), func(i int) any { return funcs[i] })
	writeJSONL(filepath.Join(*outDir, "unresolved_sites"+*suffix+".jsonl"), len(unresolved), func(i int) any { return unresolved[i] })

	sortedKeys := func(m map[string]bool) []string {
		out := make([]string, 0, len(m))
		for k := range m {
			out = append(out, k)
		}
		sort.Strings(out)
		return out
	}
	exFiles := make([]map[string]string, 0, len(excluded))
	for _, k := range func() []string {
		ks := make([]string, 0, len(excluded))
		for k := range excluded {
			ks = append(ks, k)
		}
		sort.Strings(ks)
		return ks
	}() {
		exFiles = append(exFiles, map[string]string{"file": k, "reason": excluded[k]})
	}
	if len(loadErrors) > 50 {
		loadErrors = loadErrors[:50]
	}
	meta := map[string]any{
		"repo":                           *repoName,
		"commit":                         *commit,
		"language":                       "go",
		"oracle":                         "golang.org/x/tools go/ssa static callees + go/callgraph/rta",
		"x_tools_version":                toolsVersion(),
		"go_version":                     runtime.Version(),
		"goos":                           envOr("GOOS", runtime.GOOS),
		"goarch":                         envOr("GOARCH", runtime.GOARCH),
		"cgo_enabled":                    envOr("CGO_ENABLED", "(default)"),
		"ssa_mode":                       "InstantiateGenerics",
		"tests_included":                 *withTests,
		"caller_key":                     "outermost enclosing declared function",
		"rta_roots":                      mains,
		"rta_root_count":                 len(roots),
		"load_errors":                    loadErrors,
		"n_load_errors":                  len(loadErrors),
		"analysed_files":                 sortedKeys(analysed),
		"excluded_files":                 exFiles,
		"build_constraint_ignored_files": sortedKeys(ignored),
		"counts": map[string]any{
			"analysed_files":                             len(analysed),
			"functions":                                  len(funcs),
			"functions_judged":                           nJudged,
			"functions_rta_reachable":                    len(reachedDecl),
			"edges":                                      len(elist),
			"edges_by_kind":                              kindCount,
			"edges_with_closure_callee":                  closureCallee,
			"dynamic_sites":                              len(dynSites),
			"dynamic_sites_unresolved":                   len(unresolved),
			"calls_at_package_init_level":                counts["calls_at_package_init_level"],
			"static_calls_to_external":                   counts["static_calls_to_external"],
			"site_without_position":                      counts["site_without_position"],
			"site_without_position_to_declared_function": counts["site_without_position_to_declared_function"],
			"declared_functions_missed_by_AllFunctions":  counts["declared_functions_missed_by_AllFunctions"],
			"declared_functions_without_ssa_body":        counts["declared_functions_without_ssa_body"],
			"static_rows_generic_instantiation":          counts["static_rows_generic_instantiation"],
		},
	}
	mf, err := os.Create(filepath.Join(*outDir, "meta"+*suffix+".json"))
	if err != nil {
		die(err)
	}
	enc := json.NewEncoder(mf)
	enc.SetIndent("", " ")
	enc.SetEscapeHTML(false)
	if err := enc.Encode(meta); err != nil {
		die(err)
	}
	mf.Close()
	fmt.Fprintf(os.Stderr, "files=%d functions=%d judged=%d edges=%d %v closure_callee=%d dyn_sites=%d unresolved=%d roots=%d load_errors=%d\n",
		len(analysed), len(funcs), nJudged, len(elist), kindCount, closureCallee, len(dynSites), len(unresolved), len(roots), len(loadErrors))
}

// nameTok returns the position of the called name token: the identifier for
// f(), the selector for x.f(), looking through generic instantiation f[T]().
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
		return f.Pos() // immediately-invoked literal: the `func` keyword
	}
	return fallback
}

func writeJSONL(path string, n int, at func(int) any) {
	f, err := os.Create(path)
	if err != nil {
		die(err)
	}
	defer f.Close()
	enc := json.NewEncoder(f)
	enc.SetEscapeHTML(false)
	for i := 0; i < n; i++ {
		if err := enc.Encode(at(i)); err != nil {
			die(err)
		}
	}
}

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}

func die(err error) {
	fmt.Fprintln(os.Stderr, "go-oracle:", err)
	os.Exit(1)
}

func toolsVersion() string {
	bi, ok := debug.ReadBuildInfo()
	if !ok {
		return ""
	}
	for _, d := range bi.Deps {
		if d.Path == "golang.org/x/tools" {
			return d.Version
		}
	}
	return ""
}
