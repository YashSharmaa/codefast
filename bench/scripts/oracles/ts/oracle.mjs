#!/usr/bin/env node
/**
 * TypeScript call-graph oracle for the codefast benchmark.
 *
 * Answer key = the TypeScript compiler's own resolution of every call site:
 *   - a node is a call because the AST says so (CallExpression, NewExpression,
 *     TaggedTemplateExpression);
 *   - its target is `checker.getResolvedSignature(node).declaration`, followed
 *     to the implementation when the resolved signature is an overload.
 *
 * Methodology follows the repowise-bench G4 TypeScript cell (scope to a source
 * subtree, no tests, no dependency install, caller keyed at the outermost named
 * function, `new X()` keyed at class X, callers containing a site tsc cannot
 * take to an implementation are withheld from the judged set). Independent
 * implementation; additionally writes a functions listing, call-site
 * positions, edges whose caller is a module-level variable initialiser, and a
 * class-hierarchy candidate set for calls that resolve to an interface or
 * abstract member (kind "interface", clearly marked as our own fold).
 *
 * Output format: bench/oracles/FORMAT.md.
 */
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import ts from "typescript";

const SRC_EXT = new Set([".ts", ".tsx", ".mts", ".cts"]);
const SKIP_DIRS = new Set([
  "node_modules", ".git", "dist", "build", "out", "coverage", ".next", ".turbo", ".yarn", "vendor",
]);
// Test code: only conventions that are unambiguous from the path.
const TEST_DIR = /(^|\/)(__tests__|__test__|test|tests|spec|runtime-tests)(\/|$)/;
const TEST_FILE = /\.(test|spec)\.[cm]?tsx?$/;
const isTestPath = (r) => TEST_DIR.test(r) || TEST_FILE.test(r);

function parseArgs(argv) {
  const o = { repo: null, out: null, tests: false, tsconfig: null, subdirs: [], name: "", commit: "", suffix: "" };
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i].replace(/^--/, "-");
    if (a === "-repo") o.repo = argv[++i];
    else if (a === "-out") o.out = argv[++i];
    else if (a === "-tsconfig") o.tsconfig = argv[++i];
    else if (a === "-tests") o.tests = true;
    else if (a === "-subdir") o.subdirs.push(argv[++i].replace(/\/+$/, ""));
    else if (a === "-name") o.name = argv[++i];
    else if (a === "-commit") o.commit = argv[++i];
    else if (a === "-suffix") o.suffix = argv[++i];
    else throw new Error(`unknown argument: ${argv[i]}`);
  }
  if (!o.repo || !o.out) throw new Error("usage: oracle.mjs -repo <dir> -out <dir> [-subdir <dir>]... [-tsconfig <file>] [-tests]");
  return o;
}

function walk(dir, acc) {
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    if (SKIP_DIRS.has(ent.name)) continue;
    const full = path.join(dir, ent.name);
    if (ent.isDirectory()) walk(full, acc);
    else if (ent.isFile() && SRC_EXT.has(path.extname(ent.name)) && !/\.d\.[cm]?ts$/.test(ent.name)) acc.push(full);
  }
  return acc;
}

const args = parseArgs(process.argv);
const repo = fs.realpathSync(path.resolve(args.repo));
const rel = (p) => path.relative(repo, p).split(path.sep).join("/");
const inSubdir = (r) => !args.subdirs.length || args.subdirs.some((d) => r === d || r.startsWith(d + "/"));

const allFiles = walk(repo, []).map((p) => ({ abs: p, rel: rel(p) }));
const scoped = allFiles.filter((f) => inSubdir(f.rel));
const roots = scoped.filter((f) => args.tests || !isTestPath(f.rel));
const excludedTests = scoped.filter((f) => !args.tests && isTestPath(f.rel)).map((f) => f.rel).sort();

let options = {
  target: ts.ScriptTarget.ESNext,
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  jsx: ts.JsxEmit.ReactJSX,
  allowJs: false,
  strict: false,
  allowImportingTsExtensions: true,
};
let configErrors = [];
if (args.tsconfig) {
  const host = { ...ts.sys, onUnRecoverableConfigFileDiagnostic: (d) => configErrors.push(ts.flattenDiagnosticMessageText(d.messageText, " ")) };
  const parsed = ts.getParsedCommandLineOfConfigFile(path.resolve(args.tsconfig), {}, host);
  if (parsed) {
    // Compiler options only; the file list is the walk above, so the analysed
    // set is a property of this run and not of the project's tsconfig.
    options = { ...parsed.options };
    configErrors.push(...parsed.errors.filter((d) => d.category === ts.DiagnosticCategory.Error)
      .map((d) => ts.flattenDiagnosticMessageText(d.messageText, " ")));
  }
}
// No dependencies are installed, so ambient type packages cannot be loaded.
options = { ...options, noEmit: true, skipLibCheck: true, declaration: false, composite: false, incremental: false, types: [] };

const program = ts.createProgram({ rootNames: roots.map((f) => f.abs), options });
const checker = program.getTypeChecker();

const sources = [];
for (const f of roots) {
  const sf = program.getSourceFile(f.abs);
  if (sf) sources.push({ sf, rel: f.rel });
}
const analysed = new Set(sources.map((s) => s.rel));
const relOfSf = (sf) => rel(fs.existsSync(sf.fileName) ? fs.realpathSync(sf.fileName) : sf.fileName);

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------
const isFunctionLike = (n) =>
  ts.isFunctionDeclaration(n) || ts.isMethodDeclaration(n) || ts.isConstructorDeclaration(n) ||
  ts.isGetAccessorDeclaration(n) || ts.isSetAccessorDeclaration(n) ||
  ts.isFunctionExpression(n) || ts.isArrowFunction(n);

/** Strip parentheses, `as` casts, `satisfies` and non-null assertions. */
function unwrapUp(node) {
  let n = node;
  while (n.parent && (ts.isParenthesizedExpression(n.parent) || ts.isAsExpression(n.parent) ||
    ts.isSatisfiesExpression(n.parent) || ts.isNonNullExpression(n.parent) || ts.isTypeAssertionExpression(n.parent))) {
    n = n.parent;
  }
  return n;
}

/**
 * The node a function is declared by. A function expression or arrow that is
 * the initialiser of a variable, an object-literal property or a class field is
 * declared by that statement / property; anything else by itself.
 */
function declNode(fn) {
  if (!ts.isFunctionExpression(fn) && !ts.isArrowFunction(fn)) return fn;
  const inner = unwrapUp(fn);
  const p = inner.parent;
  if (!p) return fn;
  if (ts.isVariableDeclaration(p) && p.initializer === inner) {
    const stmt = p.parent && p.parent.parent;
    if (stmt && ts.isVariableStatement(stmt) && p.parent.declarations.length === 1) return stmt;
    return p;
  }
  if ((ts.isPropertyAssignment(p) || ts.isPropertyDeclaration(p)) && p.initializer === inner) return p;
  if (ts.isExportAssignment(p) && p.expression === inner) return p;
  return fn;
}

/** Named = has a declaration a reader would call "the definition of foo". */
function isNamedFunction(n) {
  if (!isFunctionLike(n)) return false;
  if (ts.isFunctionExpression(n) || ts.isArrowFunction(n)) return declNode(n) !== n;
  return true;
}
const hasBody = (n) => !!n.body;

function lineOf(sf, pos) { return sf.getLineAndCharacterOfPosition(pos).line + 1; }
function colOf(sf, pos) { return sf.getLineAndCharacterOfPosition(pos).character + 1; }

/** Declaration line: first token of the declaration, after JSDoc and decorators. */
function declLine(node, sf) {
  let pos = node.getStart(sf, false);
  const decorators = ts.canHaveDecorators(node) ? ts.getDecorators(node) : undefined;
  if (decorators && decorators.length) {
    let i = decorators[decorators.length - 1].end;
    while (i < sf.text.length && /\s/.test(sf.text[i])) i++;
    pos = Math.max(pos, i);
  }
  return lineOf(sf, pos);
}

function propName(nameNode, sf) {
  if (!nameNode) return "<anonymous>";
  if (ts.isIdentifier(nameNode) || ts.isPrivateIdentifier(nameNode)) return nameNode.text;
  if (ts.isStringLiteralLike(nameNode) || ts.isNumericLiteral(nameNode)) return nameNode.text;
  return nameNode.getText(sf);
}

function containerName(node, sf) {
  // class member -> class name; object-literal member -> the variable or
  // property the literal is bound to, when it is bound directly.
  const p = node.parent;
  if (!p) return "";
  if (ts.isClassLike(p)) return p.name ? p.name.text : "<anonymous class>";
  if (ts.isObjectLiteralExpression(p)) {
    const up = unwrapUp(p).parent;
    if (up && (ts.isVariableDeclaration(up) || ts.isPropertyAssignment(up) || ts.isPropertyDeclaration(up))) return propName(up.name, sf);
    return "<object>";
  }
  return "";
}

function describe(fn, sf) {
  const d = declNode(fn);
  let name, recv = "", kind;
  if (ts.isConstructorDeclaration(fn)) { name = "constructor"; recv = containerName(fn, sf); kind = "constructor"; }
  else if (ts.isMethodDeclaration(fn)) { name = propName(fn.name, sf); recv = containerName(fn, sf); kind = "method"; }
  else if (ts.isGetAccessorDeclaration(fn)) { name = propName(fn.name, sf); recv = containerName(fn, sf); kind = "getter"; }
  else if (ts.isSetAccessorDeclaration(fn)) { name = propName(fn.name, sf); recv = containerName(fn, sf); kind = "setter"; }
  else if (ts.isFunctionDeclaration(fn)) { name = fn.name ? fn.name.text : "default"; kind = "function"; }
  else if (ts.isVariableStatement(d)) { name = propName(d.declarationList.declarations[0].name, sf); kind = "var_func"; }
  else if (ts.isVariableDeclaration(d)) { name = propName(d.name, sf); kind = "var_func"; }
  else if (ts.isPropertyDeclaration(d)) { name = propName(d.name, sf); recv = containerName(d, sf); kind = "field_func"; }
  else if (ts.isPropertyAssignment(d)) { name = propName(d.name, sf); recv = containerName(d, sf); kind = "property_func"; }
  else if (ts.isExportAssignment(d)) { name = "default"; kind = "var_func"; }
  else { name = "<anonymous>"; kind = "closure"; }
  return { name, recv, kind, d };
}

// id -> record. id = file \0 line \0 name (a line can hold two declarations).
const funcs = new Map();
const recOfNode = new Map(); // function-like or class node -> record
function addRecord(node, sf, r, info) {
  const line = declLine(info.d, sf);
  const rec = {
    file: r, name: info.name, line, end_line: lineOf(sf, info.d.getEnd()),
    receiver_or_class: info.recv, kind: info.kind,
    nested_in: null, judged: true, unresolved_sites: 0,
  };
  const startLine = lineOf(sf, info.d.getStart(sf, false));
  if (startLine !== line) rec.start_line = startLine;
  recOfNode.set(node, rec);
  funcs.set(`${r}\u0000${line}\u0000${info.name}\u0000${info.kind}`, rec);
  return rec;
}

function outermostNamed(node) {
  let found = null;
  for (let n = node.parent; n; n = n.parent) if (isNamedFunction(n) && hasBody(n)) found = n;
  return found;
}
function innermostNamed(node) {
  for (let n = node.parent; n; n = n.parent) if (isNamedFunction(n) && hasBody(n)) return n;
  return null;
}
function innermostFunction(node) {
  for (let n = node.parent; n; n = n.parent) if (isFunctionLike(n)) return n;
  return null;
}
/** The module-level variable whose initialiser contains `node`, if any. */
function moduleVariable(node) {
  let v = null;
  for (let n = node.parent; n; n = n.parent) {
    if (ts.isVariableDeclaration(n) && n.parent && n.parent.parent && ts.isVariableStatement(n.parent.parent) &&
        ts.isSourceFile(n.parent.parent.parent)) v = n;
  }
  return v;
}

for (const { sf, rel: r } of sources) {
  const visit = (n) => {
    if (isNamedFunction(n) && hasBody(n)) {
      addRecord(n, sf, r, describe(n, sf));
    } else if (ts.isClassDeclaration(n) || ts.isClassExpression(n)) {
      let name = n.name ? n.name.text : "";
      let d = n;
      if (!name) {
        const up = unwrapUp(n).parent;
        if (up && ts.isVariableDeclaration(up)) {
          name = propName(up.name, sf);
          const stmt = up.parent && up.parent.parent;
          d = stmt && ts.isVariableStatement(stmt) && up.parent.declarations.length === 1 ? stmt : up;
        } else name = "<anonymous class>";
      }
      addRecord(n, sf, r, { name, recv: "", kind: "class", d });
    }
    ts.forEachChild(n, visit);
  };
  ts.forEachChild(sf, visit);
}
// nested_in: the outermost named function a record is written inside.
const outerRecOf = new Map();
for (const [node, rec] of recOfNode) {
  const o = outermostNamed(node);
  if (!o) continue;
  const orec = recOfNode.get(o);
  rec.nested_in = { name: orec.name, line: orec.line };
  outerRecOf.set(rec, orec);
}

// Module-level variables that take part in an edge (as the caller of calls made
// while initialising them, or as the callee of `X(...)` / `new X(...)` when X is
// a variable holding a constructed value rather than a declared function). They
// are listed in functions.jsonl with kind "variable" so that both ends of every
// edge can be looked up, and are never "judged".
const varRecOf = new Map();
function variableRecord(vdecl) {
  let rec = varRecOf.get(vdecl);
  if (rec) return rec;
  const sf = vdecl.getSourceFile();
  const stmt = vdecl.parent.parent;
  const d = vdecl.parent.declarations.length === 1 ? stmt : vdecl;
  rec = {
    file: relOfSf(sf), name: propName(vdecl.name, sf), line: lineOf(sf, d.getStart(sf, false)),
    end_line: lineOf(sf, d.getEnd()), receiver_or_class: "", kind: "variable", nested_in: null, judged: false,
  };
  // The variable may already be listed as a function or class (its
  // initialiser is one): one position, one record.
  for (const k of ["var_func", "class", "function"]) {
    const existing = funcs.get(`${rec.file}\u0000${rec.line}\u0000${rec.name}\u0000${k}`);
    if (existing) { varRecOf.set(vdecl, existing); return existing; }
  }
  varRecOf.set(vdecl, rec);
  funcs.set(`${rec.file}\u0000${rec.line}\u0000${rec.name}\u0000variable`, rec);
  return rec;
}
/** The function record a callee expression names through a `const` binding. */
function constFunctionOf(nameNode) {
  let sym = checker.getSymbolAtLocation(nameNode);
  if (sym && sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
  const decls = (sym && sym.declarations) || [];
  if (decls.length !== 1) return null;
  const d = decls[0];
  if (!ts.isVariableDeclaration(d) || !d.initializer || !(d.parent.flags & ts.NodeFlags.Const)) return null;
  let init = d.initializer;
  while (ts.isParenthesizedExpression(init) || ts.isAsExpression(init) || ts.isSatisfiesExpression(init) || ts.isNonNullExpression(init)) init = init.expression;
  if (!isFunctionLike(init) || !init.body) return null;
  return recOfNode.get(init) || null;
}

/**
 * True when the callee expression names a parameter or a reassignable
 * (`let` / `var`) variable. tsc then reports the signature of the value the
 * binding was DECLARED with (a default argument, an initialiser), which is not
 * necessarily the function that runs: `function f(cb = () => 1) { cb(); }`.
 */
function calleeIsRebindable(nameNode) {
  if (!ts.isIdentifier(nameNode)) return false;
  let sym = checker.getSymbolAtLocation(nameNode);
  if (sym && sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
  let d = sym && sym.valueDeclaration;
  if (!d) return false;
  while (ts.isBindingElement(d)) d = d.parent.parent;
  if (ts.isParameter(d)) return true;
  return ts.isVariableDeclaration(d) && ts.isVariableDeclarationList(d.parent) && !(d.parent.flags & ts.NodeFlags.Const);
}

/** The module-level variable a callee expression names, if it names one. */
function calleeVariable(nameNode) {
  let sym = checker.getSymbolAtLocation(nameNode);
  if (sym && sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
  for (const d of (sym && sym.declarations) || []) {
    if (ts.isVariableDeclaration(d) && d.parent && d.parent.parent && ts.isVariableStatement(d.parent.parent) &&
        ts.isSourceFile(d.parent.parent.parent) && analysed.has(relOfSf(d.getSourceFile()))) return d;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Call sites
// ---------------------------------------------------------------------------
function implementationOf(decl) {
  if (decl.body || !decl.name) return decl;
  const sym = checker.getSymbolAtLocation(decl.name);
  const impl = sym && sym.declarations && sym.declarations.find((d) => d.body && isFunctionLike(d));
  return impl || decl;
}
function constructedClass(node) {
  let sym = checker.getSymbolAtLocation(node.expression);
  if (sym && sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
  const decls = (sym && sym.declarations) || [];
  return decls.find((d) => ts.isClassDeclaration(d) || ts.isClassExpression(d)) || null;
}
function calleeNameNode(n) {
  let e = ts.isTaggedTemplateExpression(n) ? n.tag : n.expression;
  while (ts.isParenthesizedExpression(e) || ts.isNonNullExpression(e) || ts.isAsExpression(e)) e = e.expression;
  if (ts.isPropertyAccessExpression(e)) return e.name;
  if (ts.isElementAccessExpression(e)) return e.argumentExpression || e;
  return e;
}

// Classes by declaring type, for the class-hierarchy candidate fold.
const classNodes = [...recOfNode.keys()].filter((n) => ts.isClassLike(n));
function classDerivesFrom(cls, targetSym, seen = new Set()) {
  if (seen.has(cls)) return false;
  seen.add(cls);
  for (const h of cls.heritageClauses || []) {
    for (const t of h.types) {
      let s = checker.getSymbolAtLocation(t.expression);
      if (s && s.flags & ts.SymbolFlags.Alias) s = checker.getAliasedSymbol(s);
      if (!s) continue;
      if (s === targetSym) return true;
      for (const d of s.declarations || []) {
        if ((ts.isClassLike(d) || ts.isInterfaceDeclaration(d)) && classDerivesFrom(d, targetSym, seen)) return true;
      }
    }
  }
  return false;
}
function chaCandidates(memberDecl) {
  // memberDecl: a bodiless member of an interface or (abstract) class.
  const owner = memberDecl.parent;
  if (!owner || !(ts.isInterfaceDeclaration(owner) || ts.isClassLike(owner)) || !memberDecl.name) return [];
  const ownerSym = owner.name ? checker.getSymbolAtLocation(owner.name) : null;
  if (!ownerSym) return [];
  const name = propName(memberDecl.name, memberDecl.getSourceFile());
  const out = [];
  for (const cls of classNodes) {
    if (cls === owner || !classDerivesFrom(cls, ownerSym)) continue;
    for (const m of cls.members) {
      if (!m.name || propName(m.name, cls.getSourceFile()) !== name) continue;
      if (isFunctionLike(m) && m.body) out.push(m);
      else if (ts.isPropertyDeclaration(m) && m.initializer) {
        let init = m.initializer;
        while (ts.isParenthesizedExpression(init) || ts.isAsExpression(init)) init = init.expression;
        if (isFunctionLike(init)) out.push(init);
      }
    }
  }
  return out;
}

const counts = {
  call_sites: 0, resolved: 0, unresolved: 0, external_target: 0, abstract_target: 0,
  module_level_sites: 0, module_level_in_variable: 0, closure_target: 0, variable_callee: 0, resolved_via_const_initializer: 0, jsx_skipped: 0,
  rebindable_callee: 0, accessor_sites: 0,
};
const edges = new Map();
const unresolvedSites = [];

function endpointOf(rec) {
  const e = { file: rec.file, name: rec.name, line: rec.line };
  if (rec.receiver_or_class) e.receiver = rec.receiver_or_class;
  return e;
}
function targetEndpoint(decl) {
  // -> { ep, closure, outer } or null when outside the analysed set.
  const tsf = decl.getSourceFile();
  const trel = relOfSf(tsf);
  if (!analysed.has(trel)) return null;
  const rec = recOfNode.get(decl);
  if (rec) return { ep: endpointOf(rec), rec };
  // A function with a body but no declaration of its own: a closure.
  const outer = outermostNamed(decl);
  const orec = outer && recOfNode.get(outer);
  return {
    ep: { file: trel, name: (orec ? orec.name : "<module>") + "$closure", line: lineOf(tsf, decl.getStart(tsf, false)) },
    closure: true, outer: orec ? endpointOf(orec) : null,
  };
}

for (const { sf, rel: r } of sources) {
  const visit = (n) => {
    if (ts.isJsxOpeningElement(n) || ts.isJsxSelfClosingElement(n)) counts.jsx_skipped++;
    if (ts.isCallExpression(n) || ts.isNewExpression(n) || ts.isTaggedTemplateExpression(n)) {
      counts.call_sites++;
      const nameNode = calleeNameNode(n);
      const sitePos = nameNode.getStart(sf, false);
      const site = { line: lineOf(sf, sitePos), col: colOf(sf, sitePos) };
      const outer = outermostNamed(n);
      const inner = innermostNamed(n);
      const callerRec = outer ? recOfNode.get(outer) : null;
      const modVar = outer ? null : moduleVariable(n);
      let caller = null, callerKind = null;
      if (callerRec) caller = endpointOf(callerRec);
      else if (modVar) { caller = endpointOf(variableRecord(modVar)); callerKind = "variable"; }
      if (!outer) { counts.module_level_sites++; if (modVar) counts.module_level_in_variable++; }
      const withhold = (kind, reason) => {
        if (callerRec) { callerRec.judged = false; callerRec.unresolved_sites++; }
        unresolvedSites.push({ caller: caller || { file: r, name: "<module>", line: 0 }, site_line: site.line, site_col: site.col, kind, reason });
      };
      const emit = (t, kind, extra = {}) => {
        if (!caller) return;
        const e = { caller, callee: t.ep, site_line: site.line, site_col: site.col, kind, ...extra };
        if (ts.isNewExpression(n)) e.via = "new";
        else if (ts.isTaggedTemplateExpression(n)) e.via = "tagged_template";
        if (callerKind) e.caller_kind = callerKind;
        if (inner && inner !== outer) { const ir = recOfNode.get(inner); e.caller_inner = { name: ir.name, line: ir.line }; }
        const innerFn = innermostFunction(n);
        if (innerFn && innerFn !== inner) e.in_closure = true;
        if (t.closure) { e.callee_closure = true; if (t.outer) e.callee_outer = t.outer; }
        const key = [e.caller.file, e.caller.line, e.caller.name, e.site_line, e.site_col, e.callee.file, e.callee.line, e.callee.name, kind].join("\u0000");
        if (!edges.has(key)) edges.set(key, e);
      };

      let sig;
      try { sig = checker.getResolvedSignature(n); } catch { sig = undefined; }
      let decl = sig && sig.declaration && !ts.isJSDocSignature(sig.declaration) ? implementationOf(sig.declaration) : null;
      let cls = ts.isNewExpression(n) ? constructedClass(n) : null;
      // `X(...)` / `new X(...)` where X is a module-level variable and tsc
      // cannot take the call to a function body: the edge to the variable is
      // still a compiler fact (symbol resolution), kept apart by callee_kind.
      const emitVariableCallee = () => {
        const v = calleeVariable(nameNode);
        if (!v) return;
        counts.variable_callee++;
        const vrec = variableRecord(v);
        emit({ ep: endpointOf(vrec) }, "static", vrec.kind === "variable" ? { callee_kind: "variable" } : { resolution: "variable-initializer" });
      };
      // A `const` whose initialiser is a function: when the variable carries a
      // type annotation, tsc resolves a call of it to the annotation's
      // signature (no body). The symbol still names exactly one function, and
      // a const cannot be rebound, so the edge is as static as any other.
      const constFn = !cls && (!decl || !decl.body) ? constFunctionOf(nameNode) : null;
      if (constFn) {
        counts.resolved_via_const_initializer++;
        emit({ ep: endpointOf(constFn), rec: constFn }, "static", { resolution: "const-initializer" });
      } else if (!decl && !cls) {
        counts.unresolved++;
        withhold("unresolved", "tsc has no declaration for the resolved signature (any-typed, untyped or missing dependency)");
        emitVariableCallee();
      } else if (cls) {
        counts.resolved++;
        const t = targetEndpoint(cls);
        if (!t) counts.external_target++;
        else {
          // `new X()` constructs X. The constructor that runs (X's own or an
          // inherited one) is an equally acceptable answer: callee_alt.
          const extra = {};
          if (decl && ts.isConstructorDeclaration(decl) && decl.body) {
            const ct = targetEndpoint(decl);
            if (ct && !ct.closure) extra.callee_alt = [ct.ep];
          }
          emit(t, "static", extra);
        }
      } else {
        counts.resolved++;
        const trel = relOfSf(decl.getSourceFile());
        if (!analysed.has(trel)) {
          counts.external_target++;
        } else if (!decl.body) {
          // Interface member, abstract member, function-typed property or
          // parameter, call signature of a type alias: tsc stops at the
          // declared signature and does not enumerate implementations.
          counts.abstract_target++;
          const isMember = (ts.isMethodSignature(decl) || ts.isMethodDeclaration(decl) || ts.isPropertySignature(decl) || ts.isPropertyDeclaration(decl)) &&
            decl.parent && (ts.isInterfaceDeclaration(decl.parent) || ts.isClassLike(decl.parent));
          const kind = isMember ? "interface" : "funcptr";
          withhold(kind, isMember ? "resolves to a bodiless interface/abstract member" : "resolves to a function type, not a function");
          if (isMember) {
            const cands = chaCandidates(decl).map((c) => targetEndpoint(c)).filter((t) => t && !t.closure);
            for (const t of cands) emit(t, "interface", { candidates: cands.length, resolution: "class-hierarchy" });
          } else emitVariableCallee();
        } else if (calleeIsRebindable(nameNode)) {
          // A call through a parameter or a `let`: the body tsc points at is
          // the binding's declared value only. Not a static edge.
          counts.rebindable_callee++;
          withhold("funcptr", "callee is a parameter or reassignable variable; tsc resolves it to its declared value only");
        } else {
          const t = targetEndpoint(decl);
          if (t.closure) counts.closure_target++;
          emit(t, "static");
        }
      }
    }
    // A property access that runs an in-repo getter or setter is an implicit
    // call. It is not written as an edge (FORMAT.md, "What is excluded"), so
    // the enclosing function's outgoing calls are not fully known: withhold it.
    if (ts.isPropertyAccessExpression(n)) {
      let sym = checker.getSymbolAtLocation(n.name);
      if (sym && sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
      const acc = ((sym && sym.declarations) || []).find((d) =>
        (ts.isGetAccessorDeclaration(d) || ts.isSetAccessorDeclaration(d)) && d.body && analysed.has(relOfSf(d.getSourceFile())));
      if (acc) {
        counts.accessor_sites++;
        const pos = n.name.getStart(sf, false);
        const outer = outermostNamed(n);
        const callerRec = outer ? recOfNode.get(outer) : null;
        const modVar = outer ? null : moduleVariable(n);
        if (callerRec) { callerRec.judged = false; callerRec.unresolved_sites++; }
        unresolvedSites.push({
          caller: callerRec ? endpointOf(callerRec) : modVar ? endpointOf(variableRecord(modVar)) : { file: r, name: "<module>", line: 0 },
          site_line: lineOf(sf, pos), site_col: colOf(sf, pos), kind: "accessor",
          reason: "property access runs an in-repo getter/setter (implicit call, not an edge)",
        });
      }
    }
    ts.forEachChild(n, visit);
  };
  ts.forEachChild(sf, visit);
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------
// Callers are keyed at the outermost named function, so that is where an
// unresolved site is charged; a nested function inherits its verdict.
for (const rec of recOfNode.values()) {
  if (rec.kind === "class") { delete rec.unresolved_sites; continue; }
  const o = outerRecOf.get(rec);
  if (o) { rec.judged = o.judged; delete rec.unresolved_sites; }
}

const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const edgeList = [...edges.values()].sort((a, b) =>
  cmp(a.caller.file, b.caller.file) || a.site_line - b.site_line || a.site_col - b.site_col ||
  cmp(a.callee.file, b.callee.file) || a.callee.line - b.callee.line || cmp(a.kind, b.kind));
const funcList = [...funcs.values()].sort((a, b) => cmp(a.file, b.file) || a.line - b.line || cmp(a.name, b.name));
unresolvedSites.sort((a, b) => cmp(a.caller.file, b.caller.file) || a.site_line - b.site_line || a.site_col - b.site_col);

const byKind = {};
for (const e of edgeList) byKind[e.kind] = (byKind[e.kind] || 0) + 1;
const fnRecs = funcList.filter((f) => f.kind !== "class" && f.kind !== "variable");
const meta = {
  repo: args.name, commit: args.commit, language: "typescript",
  oracle: `TypeScript ${ts.version} type checker (getResolvedSignature per call site)`,
  typescript_version: ts.version, node_version: process.version,
  tests_included: args.tests, subdirs: args.subdirs,
  tsconfig: args.tsconfig ? rel(path.resolve(args.tsconfig)) : null,
  config_errors: configErrors,
  caller_key: "outermost enclosing named function",
  dependencies_installed: false,
  analysed_files: [...analysed].sort(),
  excluded_files: excludedTests.map((f) => ({ file: f, reason: "test" })),
  counts: {
    analysed_files: analysed.size,
    functions: fnRecs.length,
    classes: funcList.filter((f) => f.kind === "class").length,
    variables_as_endpoints: funcList.filter((f) => f.kind === "variable").length,
    functions_top_level: fnRecs.filter((f) => !f.nested_in).length,
    functions_judged: fnRecs.filter((f) => !f.nested_in && f.judged).length,
    edges: edgeList.length, edges_by_kind: byKind,
    edges_from_module_variables: edgeList.filter((e) => e.caller_kind === "variable").length,
    edges_to_module_variables: edgeList.filter((e) => e.callee_kind === "variable").length,
    edges_with_closure_callee: edgeList.filter((e) => e.callee_closure).length,
    unresolved_sites: unresolvedSites.length,
    ...counts,
  },
};

fs.mkdirSync(args.out, { recursive: true });
const jl = (rows) => rows.map((x) => JSON.stringify(x)).join("\n") + (rows.length ? "\n" : "");
fs.writeFileSync(path.join(args.out, `edges${args.suffix}.jsonl`), jl(edgeList));
fs.writeFileSync(path.join(args.out, `functions${args.suffix}.jsonl`), jl(funcList));
fs.writeFileSync(path.join(args.out, `unresolved_sites${args.suffix}.jsonl`), jl(unresolvedSites));
fs.writeFileSync(path.join(args.out, `meta${args.suffix}.json`), JSON.stringify(meta, null, 1) + "\n");
console.error(JSON.stringify(meta.counts));
