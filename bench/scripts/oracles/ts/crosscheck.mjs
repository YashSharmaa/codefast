#!/usr/bin/env node
/**
 * Independent verifier's cross-check of the TypeScript oracle.
 *
 *   node crosscheck.mjs -repo <dir> -subdir <dir> -tsconfig <file> -oracle <oracle dir>
 *
 * The oracle takes checker.getResolvedSignature(call).declaration. This script
 * asks a different question of the same compiler, the SYMBOL the callee
 * expression names (checker.getSymbolAtLocation, aliases followed), and reports
 *   1. static oracle rows whose callee expression names something that is not a
 *      fixed binding of one function (a parameter, a `let`/`var`, a property that
 *      is assigned elsewhere): the "static" target is then only the declared or
 *      default value, not a compiler-certain target;
 *   2. calls whose symbol is a function declaration / method / const function
 *      in the analysed files but for which the oracle has no row and no
 *      unresolved entry (lost edges);
 *   3. static rows where symbol and resolved signature disagree on the target;
 *   4. property reads that resolve to an in-repo getter (implicit calls the
 *      oracle does not emit);
 *   5. static method calls whose target method is overridden in a subclass.
 */
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import ts from "typescript";

const arg = (n) => { const i = process.argv.indexOf(n); return i < 0 ? null : process.argv[i + 1]; };
const repo = fs.realpathSync(path.resolve(arg("-repo")));
const subdir = arg("-subdir").replace(/\/+$/, "");
const odir = arg("-oracle");
const rel = (p) => path.relative(repo, p).split(path.sep).join("/");
const jl = (f) => fs.readFileSync(path.join(odir, f), "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
const meta = JSON.parse(fs.readFileSync(path.join(odir, "meta.json"), "utf8"));
const analysed = new Set(meta.analysed_files);
const edges = jl("edges.jsonl"), unres = jl("unresolved_sites.jsonl"), funcs = jl("functions.jsonl");

const parsed = ts.getParsedCommandLineOfConfigFile(path.resolve(arg("-tsconfig")), {}, { ...ts.sys, onUnRecoverableConfigFileDiagnostic() {} });
const options = { ...parsed.options, noEmit: true, skipLibCheck: true, declaration: false, composite: false, incremental: false, types: [] };
const program = ts.createProgram({ rootNames: [...analysed].map((r) => path.join(repo, r)), options });
const checker = program.getTypeChecker();
const relSf = (sf) => rel(fs.existsSync(sf.fileName) ? fs.realpathSync(sf.fileName) : sf.fileName);
const lc = (sf, pos) => { const p = sf.getLineAndCharacterOfPosition(pos); return [p.line + 1, p.character + 1]; };

const siteKey = (file, line, col) => `${file}:${line}:${col}`;
const rowsAt = new Map();
for (const e of edges) { const k = siteKey(e.caller.file, e.site_line, e.site_col); if (!rowsAt.has(k)) rowsAt.set(k, []); rowsAt.get(k).push(e); }
const unresAt = new Set(unres.map((u) => siteKey(u.caller.file, u.site_line, u.site_col)));

function calleeName(n) {
  let e = ts.isTaggedTemplateExpression(n) ? n.tag : n.expression;
  while (ts.isParenthesizedExpression(e) || ts.isNonNullExpression(e) || ts.isAsExpression(e)) e = e.expression;
  if (ts.isPropertyAccessExpression(e)) return e.name;
  if (ts.isElementAccessExpression(e)) return e.argumentExpression || e;
  return e;
}
const fnLike = (n) => ts.isFunctionDeclaration(n) || ts.isMethodDeclaration(n) || ts.isConstructorDeclaration(n) || ts.isFunctionExpression(n) || ts.isArrowFunction(n) || ts.isGetAccessorDeclaration(n) || ts.isSetAccessorDeclaration(n);
function unwrap(e) { while (e && (ts.isParenthesizedExpression(e) || ts.isAsExpression(e) || ts.isSatisfiesExpression(e) || ts.isNonNullExpression(e))) e = e.expression; return e; }
/** Classify what a symbol's declaration says about the binding. */
function bindingOf(sym) {
  const ds = (sym && sym.declarations) || [];
  if (!ds.length) return { kind: "none" };
  const d = ds.find((x) => fnLike(x) && x.body) || ds[0];
  if (ts.isFunctionDeclaration(d)) return { kind: "function", decl: d };
  if (ts.isMethodDeclaration(d) && d.body) return { kind: ts.isClassLike(d.parent) ? "method" : "object-method", decl: d };
  if (ts.isMethodSignature(d) || (ts.isMethodDeclaration(d) && !d.body)) return { kind: "abstract-member" };
  if (ts.isParameter(d)) return { kind: "parameter", init: !!d.initializer };
  if (ts.isVariableDeclaration(d)) {
    const isConst = !!(d.parent.flags & ts.NodeFlags.Const);
    const init = unwrap(d.initializer);
    if (init && fnLike(init)) return { kind: isConst ? "const-function" : "let-function", decl: init };
    return { kind: isConst ? "const-value" : "let-value" };
  }
  if (ts.isPropertyDeclaration(d)) { const init = unwrap(d.initializer); return init && fnLike(init) ? { kind: ts.hasEffectiveModifier(d, ts.ModifierFlags.Readonly) || true ? "class-field-function" : "", decl: init, readonly: ts.hasEffectiveModifier(d, ts.ModifierFlags.Readonly) } : { kind: "class-field-value" }; }
  if (ts.isPropertyAssignment(d)) { const init = unwrap(d.initializer); return init && fnLike(init) ? { kind: "object-property-function", decl: init } : { kind: "object-property-value" }; }
  if (ts.isPropertySignature(d)) return { kind: "property-signature" };
  if (ts.isClassLike(d)) return { kind: "class", decl: d };
  if (ts.isBindingElement(d)) return { kind: "destructured" };
  return { kind: ts.SyntaxKind[d.kind] };
}
const FIXED = new Set(["function", "method", "object-method", "const-function", "class", "class-field-function", "object-property-function"]);

const out = { call_sites: 0, static_rows_checked: 0, by_binding: {}, not_fixed: [], lost: [], getter_reads: 0, getter_reads_in_judged: 0, getter_examples: [],
  override_sites: 0, override_sites_in_judged: 0, override_examples: [] };
const judged = new Map(funcs.map((f) => [`${f.file}:${f.line}:${f.name}`, f]));
// overrides: method declaration -> subclass methods that override it
const classes = [];
for (const r of analysed) { const sf = program.getSourceFile(path.join(repo, r)); const v = (n) => { if (ts.isClassLike(n)) classes.push(n); ts.forEachChild(n, v); }; v(sf); }
function baseClasses(cls, acc = new Set()) {
  for (const h of cls.heritageClauses || []) if (h.token === ts.SyntaxKind.ExtendsKeyword) for (const t of h.types) {
    let s = checker.getSymbolAtLocation(t.expression); if (s && s.flags & ts.SymbolFlags.Alias) s = checker.getAliasedSymbol(s);
    for (const d of (s && s.declarations) || []) if (ts.isClassLike(d) && !acc.has(d)) { acc.add(d); baseClasses(d, acc); }
  }
  return acc;
}
const overriddenBy = new Map();
for (const c of classes) { const bases = baseClasses(c); for (const m of c.members) { if (!(ts.isMethodDeclaration(m) && m.body && m.name)) continue; const nm = m.name.getText();
  for (const b of bases) for (const bm of b.members) if (ts.isMethodDeclaration(bm) && bm.body && bm.name && bm.name.getText() === nm) { if (!overriddenBy.has(bm)) overriddenBy.set(bm, []); overriddenBy.get(bm).push(m); } } }

for (const r of analysed) {
  const sf = program.getSourceFile(path.join(repo, r));
  const visit = (n) => {
    if (ts.isCallExpression(n) || ts.isNewExpression(n) || ts.isTaggedTemplateExpression(n)) {
      out.call_sites++;
      const nameNode = calleeName(n);
      const [line, col] = lc(sf, nameNode.getStart(sf, false));
      const k = siteKey(r, line, col);
      const rows = rowsAt.get(k) || [];
      let sym = checker.getSymbolAtLocation(nameNode);
      if (sym && sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
      const b = bindingOf(sym);
      const statics = rows.filter((e) => e.kind === "static" && !e.callee_kind);
      if (statics.length) {
        out.static_rows_checked += statics.length;
        const tag = b.kind + (ts.isNewExpression(n) ? " (new)" : "");
        out.by_binding[tag] = (out.by_binding[tag] || 0) + statics.length;
        if (!FIXED.has(b.kind) && !ts.isNewExpression(n)) for (const e of statics) out.not_fixed.push({ binding: b.kind, site: k, callee: `${e.callee.file}:${e.callee.line} ${e.callee.name}`, closure: !!e.callee_closure, caller_kind: e.caller_kind || "", resolution: e.resolution || "" });
        if (!ts.isNewExpression(n) && statics.length === 1 && b.decl && fnLike(b.decl)) {
          const [bl] = lc(b.decl.getSourceFile(), b.decl.getStart(b.decl.getSourceFile(), false));
          const e = statics[0];
          // overloads / declaration merging aside, the symbol's function and the signature's function should be the same node
          const sig = checker.getResolvedSignature(n);
          const sd = sig && sig.declaration;
          if (sd && fnLike(sd) && sd.body && sd !== b.decl) (out.disagree ||= []).push({ site: k, symbol: `${relSf(b.decl.getSourceFile())}:${bl}`, oracle: `${e.callee.file}:${e.callee.line} ${e.callee.name}` });
        }
        // overridden target
        const sig = checker.getResolvedSignature(n); const sd = sig && sig.declaration;
        if (sd && overriddenBy.has(sd)) { out.override_sites++; const e = statics[0]; const cf = judged.get(`${e.caller.file}:${e.caller.line}:${e.caller.name}`);
          if (cf && cf.judged) out.override_sites_in_judged++; if (out.override_examples.length < 8) out.override_examples.push({ site: k, method: e.callee.name, overrides: overriddenBy.get(sd).length }); }
      } else if (!rows.length && !unresAt.has(k)) {
        // nothing at all recorded for this site: fine if the target is external or the site is at module level outside a variable
        if (b.decl && fnLike(b.decl) && b.decl.body && analysed.has(relSf(b.decl.getSourceFile())) && FIXED.has(b.kind)) out.lost.push({ site: k, binding: b.kind });
      }
    }
    if (ts.isPropertyAccessExpression(n) && !(ts.isCallExpression(n.parent) && n.parent.expression === n)) {
      let sym = checker.getSymbolAtLocation(n.name);
      if (sym && sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
      const g = ((sym && sym.declarations) || []).find((d) => (ts.isGetAccessorDeclaration(d) || ts.isSetAccessorDeclaration(d)) && d.body && analysed.has(relSf(d.getSourceFile())));
      if (g) { out.getter_reads++; if (out.getter_examples.length < 6) { const [l, c] = lc(sf, n.name.getStart(sf, false)); out.getter_examples.push(`${r}:${l}:${c} .${n.name.text}`); } }
    }
    ts.forEachChild(n, visit);
  };
  ts.forEachChild(sf, visit);
}
const nf = {}; for (const x of out.not_fixed) { const t = `${x.binding}${x.closure ? " ->closure" : " ->named"}${x.caller_kind ? " (variable caller)" : ""}`; nf[t] = (nf[t] || 0) + 1; }
console.log(JSON.stringify({ call_sites: out.call_sites, static_rows_checked: out.static_rows_checked, static_rows_by_callee_binding: out.by_binding,
  static_rows_whose_callee_is_not_a_fixed_binding: nf, examples_not_fixed: out.not_fixed.filter((x) => x.closure || !x.binding.startsWith("const-value")).slice(0, 40),
  symbol_vs_signature_disagreements: (out.disagree || []).length, disagree_examples: (out.disagree || []).slice(0, 10),
  sites_with_fixed_in_repo_function_symbol_but_no_row_and_no_unresolved_entry: out.lost.length, lost_examples: out.lost.map((x) => x.site),
  getter_or_setter_property_accesses: out.getter_reads, getter_examples: out.getter_examples,
  static_call_sites_whose_target_method_is_overridden: out.override_sites, of_which_in_judged_callers: out.override_sites_in_judged, override_examples: out.override_examples }, null, 1));
