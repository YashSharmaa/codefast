# Writing a language extractor

An extractor turns one parsed source file into *facts*: definitions, references to non-local
names, and imports. Facts are linked by name only. Nothing in one file's facts points into
another file, so any file can be re-extracted alone and the linking happens at query time
(`src/resolve.rs`). Read `src/facts.rs` (the model), `src/extract.rs` (the `Ctx` helper) and
`src/lang/c.rs` and `src/lang/go.rs` (the reference implementations) before writing one.

## Shape

```rust
pub fn extract(lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch)
```

* Build a `Ctx::new(src, path, facts, scratch)` and walk the tree once with one shared
  `TreeCursor`, recursing with a `children(c)` helper that respects `MAX_DEPTH`.
* Map node kinds to a private `enum K` through a `KindTable` built once in a `OnceLock`.
  Never compare kind strings in the hot path. Look fields up by id (`extract::field`, `child`).
* Performance matters: the extractor runs over tens of millions of lines. No per-node heap
  allocation, no `node.parent()`, no `to_string()` of node text. Intern with `cx.name(node)`.

## Definitions (`cx.def(name, kind, node, sig_end)`)

* The definition spans `node`. The signature is `node.start_byte()..sig_end`, which is
  everything before the body: stop at the `{`, the `:` of a Python block, the `=` of an
  initialised variable.
* `parent`: index of the containing definition **in this file** (class of a method, struct of a
  field, enum of a member, outer function of a nested function). Set `qual` instead when the
  container is known only by name (Go receiver type, C++ `Class::method` defined out of line).
* Kinds: `Function` (free or nested function), `Method` (has a receiver or lives in a type),
  `Class`, `Struct`, `Union`, `Enum`, `EnumMember`, `Interface`, `Trait`, `TypeAlias`, `Field`
  (also class attributes, struct fields), `Property` (accessor-like members), `Variable`,
  `Constant`, `Macro`, `Module`, `Namespace`, `Impl` (Rust `impl T`: name it `T`).
* `type_name`: the declared type for variables, fields and parameters turned into fields, the
  return type for functions, the aliased type for aliases. Strip pointers, references, generic
  arguments and `Option<>`-style wrappers only when trivial. Use `NONE` when unsure. A type
  written with a module qualifier keeps it, in the form described under "Module-qualified
  names" below.
* `arity_min` / `arity_max`: parameter counts, excluding `self` / `this` receivers.
  `arity_max = 255` for variadics. `arity_min` excludes parameters with defaults.
* Flags (`dflag`): `DECL` (no body: prototypes, interface or abstract methods, `extern`),
  `LOCAL` (not visible outside the file: C `static`, a non-exported top-level JS/TS binding,
  Java `private`), `EXPORTED` (`pub`, `export`, Go capitalised, Java `public`), `ABSTRACT`,
  `ASYNC`, `FNPTR` (a member or variable that holds a callable), `TEST` (test functions).
* Emit definitions in source order. Mark the name node with `cx.mark(node)` so it is not also
  emitted as a reference.

## References (`cx.add_ref(name, kind, line, qual, nargs)`)

Only non-local names. Track locals with `cx.push_scope()`, `cx.declare(name, type)`,
`cx.is_local(name)`: parameters, local variables, loop, pattern and catch bindings, closure
parameters. A reference to a local emits nothing.

| Situation | Emit |
|---|---|
| `foo(a, b)` free call | `Call` name=`foo`, nargs=2 |
| `alias.foo()` where `alias` is bound by an import to a module, package or namespace | `Call` name=`foo`, qual=the **import target as written in the import statement**, plus any remaining attribute chain. `np.linalg.norm()` after `import numpy as np` gives qual=`numpy.linalg`. Go `v1.Pod{}` with `v1 "k8s.io/api/core/v1"` gives qual=`k8s.io/api/core/v1`. `ns.f()` after `import * as ns from './util'` gives qual=`./util` |
| `Type.foo()` / `Type::foo()` static or associated call (the qualifier is a type: local, imported by name, or capitalised and not an import alias) | `Call` name=`foo`, qual=`Type` (or `MODULE\|Type` when the type itself is written with a module qualifier). For a Go method expression `T.Method(recv, a)`, nargs leaves out the receiver |
| `b()` or `b` where `b` is the alias of a named import (`import { a as b } from './x'`, `from m import a as b`) | the exported name with the module: `Call` name=`a`, qual=`./x`. In types and path roots, `./x\|a` |
| a name bound by `import static a.b.C.name` (Java) | qual=`a.b\|C` for calls, member reads and path roots, unless an enclosing class declares a method of that name |
| calling or constructing a whole-module binding: `const m = require('./m'); m()`, `import m = require('m'); new m()` | `Call` / `New` name=the binding, qual=the import target |
| unqualified `foo()` inside a class in languages with implicit `this` (Java, C++, C#, Kotlin, Scala, Swift) | `Call` name=`foo`, qual=innermost enclosing class name |
| `x.foo()` where local `x` has declared type `T` | `MemberCall` name=`foo`, qual=`T` |
| `self.foo()` / `this.foo()` | `MemberCall` name=`foo`, qual=enclosing class name |
| `super.foo()` / `super().foo()` | `MemberCall` name=`foo`, qual=`^Class` (`^` means a strict ancestor of the enclosing class) |
| `x.foo()` receiver unknown | `MemberCall` name=`foo`, qual=`NONE` |
| `a.b.foo()` with `a: T` | `MemberCall` name=`foo`, qual=`T::b` (a type path, resolved through field types at query time) |
| `a.build().run()` with `a: T` | `MemberCall` name=`run`, qual=`T::build()` (a segment ending in `()` is the declared result of that method) |
| `this.svc.foo()` or bare `svc.foo()` where `svc` is a member of the enclosing class | `MemberCall` name=`foo`, qual=`Class::svc` |
| `x.f` read, `x.f = v` write | `MemberRead` / `MemberWrite`, qual as for member calls. A method read without a call (`run(obj.method)`) is a `MemberRead` too: the resolver lists it as a value use of the method |
| static read `Enum.Member`, `Class.CONST` | `MemberRead` name=the member, qual=the type (Java) or `$Name` when a class cannot be told from a constant object (JavaScript) |
| `new T(..)`, `T{..}`, struct literals | `New` name=`T`, qual=the import target when written `alias.T` |
| type annotation, generic argument, cast | `Type` name=the type's simple name, qual=the import target when written `alias.T`. In `Outer.Inner` or `Kind.Member`, where the root is not a module alias, the root is a `Type` reference as well |
| `class A(B)`, `extends B`, embedded struct | `Extends` name=`B`, enclosing def = `A`, qual=the import target when written `alias.B` |
| `implements I`, `impl Trait for T` | `Implements` name=`I` / `Trait`, enclosing def = the class or impl, qual as for `Extends` |
| `@decorator`, `#[attr]`, `@Annotation` | `Annotation` |
| free identifier read or assigned (module globals, imported names) | `Read` / `Write` |
| function or value stored into a member: `obj.cb = handler`, `{ onClick: handler }`, `.read = fn`, `Foo{Handler: handle}` | `Bind` name=`handler`, qual=`T.cb` (the slot, or `.cb` when the owner type is unknown), plus the `MemberWrite` for `cb` |
| a variable or field initialised with a function: `const f = handler`, `static create = makeThing`, `augment = this.extend` | `Bind` name=the function, qual=`.f` for a variable or `T.create` for a field. When the value is a member (`this.extend`, `T.create`), also emit its `MemberRead` on the same line: the resolver pairs the two to find the method |
| function passed as an argument: `register(handler)` | `Arg` name=`handler`, qual=`register` (C, C++ and Java). The other extractors emit the plain `Read` or `MemberRead`, which the resolver treats as a value use |
| `foo!(..)` (Rust) | `MacroCall` |
| each imported symbol | `Import` name=symbol, qual=module path (in addition to the `Import` record) |

### Receiver type paths

A receiver type path is a root followed by zero or more segments separated by `::`.

Roots:

* `T`: a type by name. The resolver looks the name up from the file that wrote it: defined in
  the file, imported by name, defined in the same package, or reachable through a wildcard
  import. A name that is none of these is an external type and resolves to nothing.
* `$name`: a non-local variable (a module global, an imported value, a namespace used as a
  value). The resolver reads its declared type, or treats it as a container when it has none.
* `func()`: the declared result of a function called without a receiver. When no function of
  that name is in scope the resolver tries a type of that name, which covers conversions
  such as Go's `ErrorList(xs).First()`.

A capitalised root that turns out to be a constant object needs no special care: when no type
`Name` is visible from the file, the resolver tries the value `$Name`. A type whose own name
starts with `$` is written as it is, and the resolver tries the type before the variable.

In Go a capitalised name can be a package-level variable, so the rule "capitalised and not an
import alias" does not identify a type. The Go extractor writes the bare `T` only for types
declared in the same file and for the pointer form `(*T).Method`. Everything else is a member
access on `$T`, which resolves to the type when `T` is one.

Segments:

* `::field`: the declared type of that field or property.
* `::method()`: the declared result of that method.
* `::Inner`: a type nested in a type, a namespace nested in a namespace, or an enum variant
  with fields of its own. A `type_name` may be such a path too (`Outer::Inner`,
  `module|Outer::Inner`).

Give up (`NONE`) rather than guess.

### Module-qualified names

Type names are not unique across packages, so a name written with a module qualifier must keep
it. Wherever a root of a type path or a `type_name` was written through an import alias, emit

```
TARGET|Name
```

where `TARGET` is the import target as written in the import statement (the same text the
qualified `Call` rule uses) and `|` separates it from the name. The qualifier applies to the
root only.

| Source | Emit |
|---|---|
| Go field `Tags *strset.Set` with `import "github.com/scylladb/go-set/strset"` | `type_name` = `github.com/scylladb/go-set/strset\|Set` |
| Go `dec := json.NewDecoder(r)` then `dec.Decode(&v)` | `MemberCall` `Decode`, qual=`encoding/json\|NewDecoder()` |
| Go `cfg.Default.Load()` where `cfg` is an imported package | `MemberCall` `Load`, qual=`IMPORT_PATH\|$Default` |
| Python `u: models.User` after `import app.models as models` | type `app.models\|User` |
| TypeScript `x: core.$ZodType` after `import * as core from "../core/index.js"` | type `../core/index.js\|$ZodType` |
| Java `java.util.List<String> xs` written fully qualified | type `java.util\|List` |
| Rust `let s: store::Index` after `use crate::store` | type `crate::store\|Index` |

When the alias is not a known import (the package name could not be tied to an import of the
file), use the alias text itself as the target. An unqualified name stays bare.

A member call on a module-level object of another module follows the same rule:
`core.globalRegistry.get(x)` is `MemberCall` `get`, qual=`../core/index.js|$globalRegistry`.

Rules of thumb:

* The enclosing definition is taken from `cx.push_def` / `cx.pop_def`: push every function,
  method, class, and also variables while their initialiser is visited.
* Infer a local's type from an explicit annotation or declared type, from a constructor call
  (`x = Foo()` in Python when `Foo` looks like a class, `let x = Foo::new()` gives `Foo`,
  `var x = new Foo()` gives `Foo`), from a call result (`x := pkg.NewFoo()` gives
  `IMPORT_PATH|NewFoo()`, `x = make()` gives `make()`), and from `self`.
* Do not emit references for language keywords, literals, or well-known builtins
  (`print`, `len`, `console`, `String`, `int`, ...): keep a small stop list per language.
* A name that is both declared and used on one line is still a reference only where it is used.

## Imports (`cx.add_import(target, name, alias, line, flags)`)

One record per imported module or symbol. `target` is the module path as written
(`a.b.c`, `./util`, `net/http`, `crate::foo::bar`), `name` the imported symbol or `NONE` for a
whole-module import, `alias` the local binding when it differs.

| Source | Record |
|---|---|
| `import { a as b } from './x'` | target=`./x`, name=`a`, alias=`b` |
| `import * as ns from './x'` | target=`./x`, name=`NONE`, alias=`ns` |
| `export * from './x'` | target=`./x`, name=`NONE`, alias=`NONE`, flags=`WILDCARD\|REEXPORT` |
| `export * as ns from './x'` | target=`./x`, name=`NONE`, alias=`ns`, flags=`REEXPORT` |
| `export { a as b } from './x'` | target=`./x`, name=`a`, alias=`b`, flags=`REEXPORT` |
| `import d from './x'` | target=`./x`, name=`default`, alias=`d` |
| `export { a as b }` with no source, `export default a`, `export default class A`, `module.exports = a` | target=the empty string, name=`a`, alias=`b` or `default`, flags=`REEXPORT`. An empty target means this very file |
| `export default function () {}`, `module.exports = function () {}` | no import record: a definition named `default` with `EXPORTED` |
| `const z = { ...nsA, ...nsB }` at module level, where `nsA` and `nsB` are namespace imports | one record per namespace: target=that namespace's target, name=`NONE`, alias=`z`. `z.f()` is then looked up in each module |
| `from .x import *` | target=`.x`, name=`NONE`, flags=`WILDCARD` |
| `pub use crate::a::B` | target=`crate::a`, name=`B`, flags=`REEXPORT` |
| Java `package a.b` | not an import: emit a `Namespace` definition named `a.b` as the first definition of the file |

Other flags: `SYSTEM` for standard library and external packages when syntactically evident.
Declare imported aliases so that `alias.func()` becomes a qualified `Call` rather than a
member call on an unknown receiver: keep a per-file set of import aliases in the walker.

The resolver follows re-exports exactly, so the flags matter: a name a barrel file re-exports
resolves to the file that defines it.

## Testing

* `cargo run --release -- debug-facts FILE` prints the facts of one file.
* Put fixtures in `tests/fixtures/<lang>/`, check the dump by hand, then bless the snapshot:
  `CODEFAST_BLESS=1 cargo test --test extract_snapshots`.
* Index a large real repository and spot-check: `codefast --root REPO find NAME`,
  `show NAME`, `callers NAME`, `callees NAME` against what the source says.
* Where a compiler oracle exists (`bench/oracles/`), score the exported call graph with
  `bench/scripts/oracles/score.py` and read the `contradicted` rows: each one is a wrong edge.
