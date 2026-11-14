//! `codefast map`: an overview of a repository's architecture, asked for and never pushed.
//!
//! The files are the nodes of a graph. Two files are linked by the resolved calls between their
//! definitions (the edges `export-edges` writes, direct calls only) and, more weakly, by an
//! import of one by the other. Louvain modularity optimisation splits that graph into
//! communities. Each community gets a name from its paths, never an invented word. Around that:
//! the definitions everything goes through (hubs), calls between communities that are rare
//! enough to be unexpected, and groups of files that import each other in a circle.
//!
//! Everything here is deterministic. Files are numbered in path order, every loop visits them
//! in that order, and every tie is broken by that order. The same index gives the same bytes.
//!
//! One computation, three renderings: a text overview inside the token budget, the same as
//! JSON, and a self-contained HTML page that draws the community graph.

use super::PathFilter;
use super::export::expand;
use super::refs::callee_edges;
use crate::facts::{DefKind, dflag, fflag};
use crate::lang::Lang;
use crate::output::{Out, count, est_tokens};
use crate::resolve::{Resolver, same_language_family};
use crate::store::{DefId, FileId, Index};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Weight of an import between two files, against 1 for each resolved call between them.
///
/// An import is weaker evidence than a call. It is written once however much is used, it may
/// bring in types or constants only, and a C header is included by files that call nothing in
/// it. But files that share types and no calls (a header and its users, a module of data types)
/// belong together, and without imports they would come out isolated. Half a call lets an import
/// decide where nothing else does and lets calls win wherever there are a few. An import that
/// names several files (a Go package, a header found twice) shares this weight among them, so
/// that a large package does not outweigh one call.
const IMPORT_WEIGHT: f64 = 0.5;

/// Communities drawn in the HTML page. A page with more stays readable only as a list.
pub const DRAW_MAX: usize = 300;
/// Links between drawn communities kept in the page, the heaviest first.
const LINKS_MAX: usize = 3000;
/// Files listed per community in the page.
const FILES_MAX: usize = 150;
/// Most used definitions kept per community (three are printed).
const TOP_MAX: usize = 10;
const HUBS_MAX: usize = 10;
const COUPLINGS_MAX: usize = 5;
const CYCLES_MAX: usize = 10;
/// The hubs a coupling may reach to earn its bonus: the most used definitions of the repository,
/// this share of all the definitions anything uses, and at least `HUBS_MAX`.
const HUB_SHARE: f64 = 0.01;
/// Above this many files an import cycle's shortest circle is searched from this many of its
/// files (the first in path order) only.
const CYCLE_STARTS: usize = 2000;
/// Calls between two communities that are few enough to be unexpected even when they are all
/// the two exchange.
const FEW_CALLS: u64 = 3;
/// Files with a neighbour needed before hub files are held out of the first partition.
const HUB_FILES_MIN: usize = 100;

pub struct Opts {
    pub tests: bool,
}

/// A definition and how much other files use it.
#[derive(Clone, Debug)]
pub struct Used {
    pub name: String,
    pub path: String,
    pub line: u32,
    /// Resolved calls from other files.
    pub calls: u64,
    /// Other files those calls come from.
    pub files: u32,
    pub community: usize,
}

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub name: String,
    pub path: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct Community {
    pub name: String,
    /// Paths, sorted.
    pub files: Vec<String>,
    pub defs: usize,
    pub top: Vec<Used>,
    /// (community, calls from this one into it), most first.
    pub calls_into: Vec<(usize, u64)>,
}

#[derive(Clone, Debug)]
pub struct Coupling {
    pub from: usize,
    pub to: usize,
    pub caller: Endpoint,
    pub callee: Endpoint,
    /// Resolved calls between the two communities, both ways.
    pub between: u64,
    pub cross_dir: bool,
    pub small_to_hub: bool,
    pub score: f64,
}

#[derive(Clone, Debug)]
pub struct Cycle {
    /// Files that import each other (the strongly connected component).
    pub files: usize,
    /// The shortest circle found: each file with the line of its import of the next one. The
    /// last step imports the first.
    pub steps: Vec<(String, u32)>,
    /// The search for the shortest circle started from only some of the files.
    pub partial: bool,
}

pub struct Map {
    pub files: usize,
    /// Resolved calls between definitions of different files.
    pub calls: u64,
    /// Pairs of files where one imports the other.
    pub imports: u64,
    pub communities: Vec<Community>,
    /// Files with no resolved call or import to or from another file.
    pub isolated: Vec<String>,
    pub hubs: Vec<Used>,
    pub couplings: Vec<Coupling>,
    /// Import cycles found in all, of which `cycles` holds the largest.
    pub n_cycles: usize,
    pub cycles: Vec<Cycle>,
    /// (community, community, resolved calls between them both ways), for the drawing.
    pub links: Vec<(usize, usize, u64)>,
    pub timing: Vec<(&'static str, Duration)>,
}

/// What one file contributes, gathered in parallel.
#[derive(Default)]
struct FileOut {
    /// (caller, callee): resolved direct calls from a definition of this file to one of another
    /// file, each pair once.
    calls: Vec<(DefId, DefId)>,
    /// (line, node, share): the files this file imports, with the share of an import's weight.
    imports: Vec<(u32, u32, f32)>,
    defs: u32,
}

/// The files of the map: live code files of the product (tests too with `--tests`), within the
/// path filter, in path order. Vendored, generated and documentation files are not part of the
/// architecture.
fn map_files(ix: &Index, pf: &PathFilter, tests: bool) -> Vec<FileId> {
    let mut files: Vec<FileId> = ix
        .files()
        .filter(|&f| {
            let rec = ix.file(f);
            ix.lang(f) != Lang::Other
                && rec.flags & (fflag::VENDOR | fflag::GENERATED | fflag::DOC) == 0
                && (tests || rec.flags & fflag::TEST == 0)
                && pf.matches(ix.path(f))
        })
        .collect();
    files.sort_by(|&a, &b| ix.path(a).cmp(ix.path(b)));
    files
}

fn scan_file(ix: &Index, rs: &Resolver, f: FileId, node_of: &[u32], tests: bool) -> FileOut {
    let mut out = FileOut::default();
    // A call or an import that crosses languages (a C macro taken for a Python function of the
    // same name) is a match of names, not a link between the two files.
    let lang = ix.lang(f);
    let keep = |t: DefId| -> bool {
        let v = ix.def(t);
        let g = v.file();
        g != f && node_of[g.0 as usize] != u32::MAX && (tests || v.flags() & dflag::TEST == 0) && one_family(lang, ix.lang(g))
    };
    // Lines of test code inside a product file (a Rust `mod tests`): its imports are not the
    // product's.
    let mut test_ranges: Vec<(u32, u32)> = Vec::new();
    for d in ix.defs_of_file(f) {
        if !tests && d.flags() & dflag::TEST != 0 {
            test_ranges.push((d.rec.line, d.rec.end_line));
            continue;
        }
        out.defs += 1;
        // The callers `export-edges` takes: functions, methods and accessors with a body.
        let caller =
            !d.is_decl() && (matches!(d.kind(), DefKind::Function | DefKind::Method) || (d.kind() == DefKind::Property && d.rec.end_line > d.rec.line));
        if !caller {
            continue;
        }
        let first = out.calls.len();
        for c in callee_edges(rs, &d) {
            // Direct calls only: no callback hand-overs, no matches by name, no dynamic dispatch.
            if c.passed_to.is_some() || c.by_name {
                continue;
            }
            let mut targets: Vec<DefId> = Vec::new();
            if c.member {
                targets.extend(c.targets.first().copied());
            } else if let Some(&t) = c.targets.first() {
                let mut seen = FxHashSet::default();
                expand(ix, rs, t, 0, &mut seen, &mut targets);
            }
            for t in targets {
                if keep(t) && !out.calls[first..].contains(&(d.id, t)) {
                    out.calls.push((d.id, t));
                }
            }
        }
    }
    for (line, g) in rs.import_files(f) {
        let n = node_of[g.0 as usize];
        if n != u32::MAX && one_family(lang, ix.lang(g)) && !test_ranges.iter().any(|&(a, b)| a <= line && line <= b) {
            out.imports.push((line, n, 0.0));
        }
    }
    // An import that names several files shares its weight among them.
    let mut per_line: FxHashMap<u32, u32> = FxHashMap::default();
    for &(line, _, _) in &out.imports {
        *per_line.entry(line).or_default() += 1;
    }
    for im in &mut out.imports {
        im.2 = 1.0 / per_line[&im.0] as f32;
    }
    out
}

/// Languages that call into each other's files: the resolver's families, with C, C++ and
/// Objective-C as one (a header is often taken for either).
fn one_family(a: Lang, b: Lang) -> bool {
    let c = |l: Lang| matches!(l, Lang::C | Lang::Cpp | Lang::ObjC);
    same_language_family(a, b) || (c(a) && c(b))
}

/// Every file scanned on every core. Each worker has a resolver of its own (its caches are not
/// shared between threads) and takes the next few files when it is done with the last. The
/// results are put back in file order, so the outcome does not depend on the scheduling.
fn scan(ix: &Index, nodes: &[FileId], node_of: &[u32], tests: bool) -> Vec<FileOut> {
    const CHUNK: usize = 16;
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, Vec<FileOut>)>> = Mutex::new(Vec::new());
    let workers = rayon::current_num_threads().max(1).min(nodes.len().div_ceil(CHUNK).max(1));
    rayon::scope(|s| {
        for _ in 0..workers {
            s.spawn(|_| {
                let rs = Resolver::new(ix);
                let mut mine = Vec::new();
                loop {
                    let start = next.fetch_add(CHUNK, Ordering::Relaxed);
                    if start >= nodes.len() {
                        break;
                    }
                    let end = (start + CHUNK).min(nodes.len());
                    mine.push((start, nodes[start..end].iter().map(|&f| scan_file(ix, &rs, f, node_of, tests)).collect()));
                }
                done.lock().unwrap().extend(mine);
            });
        }
    });
    let mut parts = done.into_inner().unwrap();
    parts.sort_by_key(|p| p.0);
    parts.into_iter().flat_map(|p| p.1).collect()
}

// ---------------------------------------------------------------------------------------------
// Graph algorithms
// ---------------------------------------------------------------------------------------------

/// A weighted undirected graph: neighbours in ascending order, no self loops in the lists, the
/// weight of a node's self loop kept apart.
struct Graph {
    off: Vec<usize>,
    nb: Vec<u32>,
    w: Vec<f64>,
    selfw: Vec<f64>,
}

impl Graph {
    /// `edges` hold each undirected edge once, `u != v`, in any order. Repeats add up.
    fn new(n: usize, edges: &mut Vec<(u32, u32, f64)>, selfw: Vec<f64>) -> Graph {
        for e in edges.iter_mut() {
            if e.0 > e.1 {
                std::mem::swap(&mut e.0, &mut e.1);
            }
        }
        edges.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        let mut merged: Vec<(u32, u32, f64)> = Vec::with_capacity(edges.len());
        for &(u, v, w) in edges.iter() {
            match merged.last_mut() {
                Some(l) if l.0 == u && l.1 == v => l.2 += w,
                _ => merged.push((u, v, w)),
            }
        }
        let mut deg = vec![0usize; n + 1];
        for &(u, v, _) in &merged {
            deg[u as usize] += 1;
            deg[v as usize] += 1;
        }
        let mut off = vec![0usize; n + 1];
        for i in 0..n {
            off[i + 1] = off[i] + deg[i];
        }
        let mut fill = off.clone();
        let mut nb = vec![0u32; off[n]];
        let mut w = vec![0f64; off[n]];
        // Edges are sorted by (u, v): each list receives its neighbours in ascending order, the
        // smaller ones (as `v`) before the larger ones (as `u`).
        for &(u, v, x) in &merged {
            let (a, b) = (u as usize, v as usize);
            nb[fill[b]] = u;
            w[fill[b]] = x;
            fill[b] += 1;
            let _ = a;
        }
        for &(u, v, x) in &merged {
            let a = u as usize;
            nb[fill[a]] = v;
            w[fill[a]] = x;
            fill[a] += 1;
        }
        Graph { off, nb, w, selfw }
    }

    fn n(&self) -> usize {
        self.off.len() - 1
    }

    fn degree_weight(&self, i: usize) -> f64 {
        self.w[self.off[i]..self.off[i + 1]].iter().sum::<f64>() + 2.0 * self.selfw[i]
    }
}

/// One level of Louvain: every node, in order, moves to the neighbouring community with the
/// largest gain in modularity, until a pass moves none. Returns each node's community and
/// whether anything moved.
fn one_level(g: &Graph) -> (Vec<u32>, bool) {
    const EPS: f64 = 1e-10;
    const MAX_PASSES: usize = 64;
    let n = g.n();
    let k: Vec<f64> = (0..n).map(|i| g.degree_weight(i)).collect();
    let m2: f64 = k.iter().sum();
    let mut comm: Vec<u32> = (0..n as u32).collect();
    if m2 <= 0.0 {
        return (comm, false);
    }
    let mut tot = k.clone();
    let mut wbuf = vec![0f64; n];
    let mut stamp = vec![usize::MAX; n];
    let mut touched: Vec<u32> = Vec::new();
    let mut any = false;
    for _ in 0..MAX_PASSES {
        let mut moved = false;
        for i in 0..n {
            let ci = comm[i];
            touched.clear();
            for e in g.off[i]..g.off[i + 1] {
                let c = comm[g.nb[e] as usize];
                if stamp[c as usize] != i {
                    stamp[c as usize] = i;
                    wbuf[c as usize] = 0.0;
                    touched.push(c);
                }
                wbuf[c as usize] += g.w[e];
            }
            tot[ci as usize] -= k[i];
            let gain = |c: u32| -> f64 {
                let inside = if stamp[c as usize] == i { wbuf[c as usize] } else { 0.0 };
                inside - tot[c as usize] * k[i] / m2
            };
            // Staying wins a tie, then the lowest community number.
            let mut best = ci;
            let mut best_gain = gain(ci);
            for &c in &touched {
                if c == ci {
                    continue;
                }
                let x = gain(c);
                if x > best_gain + EPS || (best != ci && (x - best_gain).abs() <= EPS && c < best) {
                    best = c;
                    best_gain = x;
                }
            }
            tot[best as usize] += k[i];
            if best != ci {
                comm[i] = best;
                moved = true;
            }
        }
        if !moved {
            break;
        }
        any = true;
    }
    (comm, any)
}

/// Community numbers 0.. in the order of each community's first node.
fn renumber(comm: &[u32]) -> (Vec<u32>, usize) {
    let mut map: FxHashMap<u32, u32> = FxHashMap::default();
    let out = comm
        .iter()
        .map(|&c| {
            let next = map.len() as u32;
            *map.entry(c).or_insert(next)
        })
        .collect();
    (out, map.len())
}

/// Louvain modularity optimisation (resolution 1) on a weighted undirected graph with `n`
/// nodes. Returns a community number per node, numbered in the order of each community's first
/// node. Nodes without edges stay alone.
pub fn louvain(n: usize, edges: &[(u32, u32, f64)]) -> Vec<u32> {
    let mut e: Vec<(u32, u32, f64)> = edges.iter().copied().filter(|e| e.0 != e.1).collect();
    let mut g = Graph::new(n, &mut e, vec![0.0; n]);
    let mut member: Vec<u32> = (0..n as u32).collect();
    loop {
        let (comm, moved) = one_level(&g);
        if !moved {
            break;
        }
        let (comm, k) = renumber(&comm);
        for m in member.iter_mut() {
            *m = comm[*m as usize];
        }
        // The communities become the nodes of the next level.
        let mut selfw = vec![0f64; k];
        let mut edges: Vec<(u32, u32, f64)> = Vec::new();
        for i in 0..g.n() {
            let ci = comm[i];
            selfw[ci as usize] += g.selfw[i];
            for x in g.off[i]..g.off[i + 1] {
                let j = g.nb[x] as usize;
                if j <= i {
                    continue;
                }
                let cj = comm[j];
                if ci == cj {
                    selfw[ci as usize] += g.w[x];
                } else {
                    edges.push((ci, cj, g.w[x]));
                }
            }
        }
        g = Graph::new(k, &mut edges, selfw);
    }
    renumber(&member).0
}

/// Strongly connected components of a directed graph (Tarjan, without recursion). `adj[i]`
/// lists the successors of node `i`. Components of one node are left out. Each component's
/// nodes are sorted, and components come in the order of their smallest node.
pub fn strong_components(adj: &[Vec<u32>]) -> Vec<Vec<u32>> {
    let n = adj.len();
    const UNSEEN: u32 = u32::MAX;
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut next = 0u32;
    let mut out: Vec<Vec<u32>> = Vec::new();
    // (node, position in its successor list)
    let mut work: Vec<(u32, usize)> = Vec::new();
    for root in 0..n as u32 {
        if index[root as usize] != UNSEEN {
            continue;
        }
        work.push((root, 0));
        index[root as usize] = next;
        low[root as usize] = next;
        next += 1;
        stack.push(root);
        on_stack[root as usize] = true;
        while let Some(&mut (v, ref mut pos)) = work.last_mut() {
            let vu = v as usize;
            if *pos < adj[vu].len() {
                let w = adj[vu][*pos];
                *pos += 1;
                let wu = w as usize;
                if index[wu] == UNSEEN {
                    index[wu] = next;
                    low[wu] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[wu] = true;
                    work.push((w, 0));
                } else if on_stack[wu] {
                    low[vu] = low[vu].min(index[wu]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent as usize] = low[parent as usize].min(low[vu]);
            }
            if low[vu] == index[vu] {
                let mut comp = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack[w as usize] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                if comp.len() > 1 {
                    comp.sort_unstable();
                    out.push(comp);
                }
            }
        }
    }
    out.sort_by_key(|c| c[0]);
    out
}

/// The shortest cycle inside one strongly connected component: a breadth-first search from
/// each of its nodes in order (from the first `starts` only), kept to the component. Returns
/// the nodes of the cycle, starting with the node the search started from, and whether the
/// search was partial. Ties go to the earlier start and the smaller neighbours.
pub fn shortest_cycle(adj: &[Vec<u32>], comp: &[u32], starts: usize) -> (Vec<u32>, bool) {
    let inside: FxHashSet<u32> = comp.iter().copied().collect();
    let mut best: Vec<u32> = Vec::new();
    let mut prev: FxHashMap<u32, u32> = FxHashMap::default();
    let mut queue: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    for &s in comp.iter().take(starts) {
        if best.len() == 2 {
            break;
        }
        prev.clear();
        queue.clear();
        queue.push_back(s);
        prev.insert(s, s);
        let mut found: Option<u32> = None;
        'bfs: while let Some(v) = queue.pop_front() {
            for &w in &adj[v as usize] {
                if !inside.contains(&w) {
                    continue;
                }
                if w == s {
                    found = Some(v);
                    break 'bfs;
                }
                if let std::collections::hash_map::Entry::Vacant(e) = prev.entry(w) {
                    e.insert(v);
                    queue.push_back(w);
                }
            }
        }
        if let Some(last) = found {
            let mut path = vec![last];
            let mut x = last;
            while x != s {
                x = prev[&x];
                path.push(x);
            }
            path.reverse();
            if best.is_empty() || path.len() < best.len() {
                best = path;
            }
        }
    }
    (best, comp.len() > starts)
}

// ---------------------------------------------------------------------------------------------
// The computation
// ---------------------------------------------------------------------------------------------

/// The directory part of a path: everything before the last `/`, empty for a file at the root.
fn dir_of(p: &str) -> &str {
    p.rfind('/').map(|i| &p[..i]).unwrap_or("")
}

fn top_dir(p: &str) -> &str {
    match p.find('/') {
        Some(i) => &p[..i],
        None => ".",
    }
}

/// The first two directories of a path (one for a file directly below the top).
fn top_two(p: &str) -> &str {
    let d = dir_of(p);
    match d.match_indices('/').nth(1) {
        Some((i, _)) => &d[..i],
        None if d.is_empty() => ".",
        None => d,
    }
}

/// The value that occurs most often (ties: the first in sorted order).
fn majority<'a>(it: impl Iterator<Item = &'a str>) -> String {
    let mut v: Vec<&str> = it.collect();
    v.sort_unstable();
    let mut best: (&str, usize) = ("", 0);
    for run in v.chunk_by(|a, b| a == b) {
        if run.len() > best.1 {
            best = (run[0], run.len());
        }
    }
    best.0.to_string()
}

/// The longest directory prefix the paths share. One file in ten may lie outside it (rounded
/// down, so a community of fewer than ten files has none): a core of forty files with one
/// stray example elsewhere is still named after its directory and not after the root. A
/// single file is its own path. Empty when no directory holds enough of the files.
fn common_dir(paths: &[&str]) -> String {
    dir_holding(paths, paths.len() - paths.len() / 10)
}

/// The deepest directory that holds at least `need` of the paths (a single path: itself).
fn dir_holding(paths: &[&str], need: usize) -> String {
    if paths.len() == 1 {
        return paths[0].to_string();
    }
    let mut under: FxHashMap<&str, usize> = FxHashMap::default();
    for p in paths {
        let d = dir_of(p);
        let mut end = 0;
        while end < d.len() {
            end = d[end..].find('/').map(|i| end + i).unwrap_or(d.len());
            *under.entry(&d[..end]).or_default() += 1;
            end += 1;
        }
    }
    // The deepest prefix that holds enough files, then the one holding more, then the first.
    let depth = |s: &str| s.bytes().filter(|&b| b == b'/').count();
    under
        .into_iter()
        .filter(|&(_, n)| n >= need)
        .max_by(|a, b| depth(a.0).cmp(&depth(b.0)).then(a.1.cmp(&b.1)).then(b.0.cmp(a.0)))
        .map(|(d, _)| d.to_string())
        .unwrap_or_default()
}

/// A short method that reads or sets one value: `getX`, `is_ready`, or a method named after a
/// field of its own type. Not what a reader means by "what everything goes through".
fn trivial_accessor(ix: &Index, d: DefId) -> bool {
    let v = ix.def(d);
    if !matches!(v.kind(), DefKind::Function | DefKind::Method | DefKind::Property) || v.rec.end_line.saturating_sub(v.rec.line) > 2 {
        return false;
    }
    let name = v.name();
    let lower: Vec<u8> = name.iter().map(|b| b.to_ascii_lowercase()).collect();
    for p in [&b"get"[..], b"set", b"is", b"has"] {
        if lower.starts_with(p) && name.len() > p.len() && (name[p.len()] == b'_' || name[p.len()].is_ascii_uppercase()) {
            return true;
        }
    }
    if v.kind() == DefKind::Property {
        return true;
    }
    let Some(owner) = v.parent_name() else { return false };
    ix.defs_of_file(v.file()).any(|o| o.id != d && matches!(o.kind(), DefKind::Field | DefKind::Property) && o.name() == name && o.parent_name() == Some(owner))
}

/// Files held out of the first partition: (hubs, files whose neighbours are all hubs). A hub
/// is a file with more neighbours than 99% of the files that have any, once at least
/// `HUB_FILES_MIN` files have one.
fn held_out(g: &Graph, degree: &[u32]) -> (Vec<bool>, Vec<bool>) {
    let n = g.n();
    let mut hub = vec![false; n];
    let mut d: Vec<u32> = degree.iter().copied().filter(|&x| x > 0).collect();
    if d.len() >= HUB_FILES_MIN {
        d.sort_unstable();
        let threshold = d[(d.len() * 99 / 100).saturating_sub(1)];
        for i in 0..n {
            hub[i] = degree[i] > threshold;
        }
    }
    let stranded = (0..n).map(|i| !hub[i] && g.off[i] < g.off[i + 1] && g.nb[g.off[i]..g.off[i + 1]].iter().all(|&j| hub[j as usize])).collect();
    (hub, stranded)
}

/// The community node `i` has most weight to among its neighbours that have one (ties: the
/// lowest number).
fn heaviest_community(g: &Graph, comm: &[u32], i: usize, none: u32) -> Option<u32> {
    let mut to: Vec<(u32, f64)> = Vec::new();
    for e in g.off[i]..g.off[i + 1] {
        let c = comm[g.nb[e] as usize];
        if c == none {
            continue;
        }
        match to.iter_mut().find(|x| x.0 == c) {
            Some(x) => x.1 += g.w[e],
            None => to.push((c, g.w[e])),
        }
    }
    to.into_iter().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0))).map(|x| x.0)
}

pub fn compute(ix: &Index, pf: &PathFilter, opts: &Opts) -> Map {
    let t0 = Instant::now();
    let mut timing: Vec<(&'static str, Duration)> = Vec::new();
    let mut lap = {
        let mut last = Instant::now();
        move |name: &'static str, timing: &mut Vec<(&'static str, Duration)>| {
            let now = Instant::now();
            timing.push((name, now - last));
            last = now;
        }
    };
    let nodes = map_files(ix, pf, opts.tests);
    let n = nodes.len();
    let mut node_of = vec![u32::MAX; ix.n_files() as usize];
    for (i, &f) in nodes.iter().enumerate() {
        node_of[f.0 as usize] = i as u32;
    }
    let outs = scan(ix, &nodes, &node_of, opts.tests);
    lap("calls and imports", &mut timing);

    // ----- the file graph -----
    let node_of_def = |d: DefId| node_of[ix.def(d).file().0 as usize];
    // Calls per file pair, and per definition from how many files and how often.
    let mut pair_keys: Vec<u64> = Vec::new();
    let mut used: FxHashMap<DefId, (u64, u32)> = FxHashMap::default();
    let mut total_calls = 0u64;
    for (i, fo) in outs.iter().enumerate() {
        let mut callees: Vec<DefId> = Vec::with_capacity(fo.calls.len());
        for &(_, t) in &fo.calls {
            let j = node_of_def(t);
            let (a, b) = if (i as u32) < j { (i as u32, j) } else { (j, i as u32) };
            pair_keys.push(((a as u64) << 32) | b as u64);
            callees.push(t);
        }
        total_calls += fo.calls.len() as u64;
        callees.sort_unstable();
        let mut k = 0;
        while k < callees.len() {
            let mut e = k;
            while e < callees.len() && callees[e] == callees[k] {
                e += 1;
            }
            let u = used.entry(callees[k]).or_default();
            u.0 += (e - k) as u64;
            u.1 += 1;
            k = e;
        }
    }
    pair_keys.sort_unstable();
    let mut pair_calls: FxHashMap<(u32, u32), u64> = FxHashMap::default();
    let mut weights: Vec<(u32, u32, f64)> = Vec::new();
    {
        let mut k = 0;
        while k < pair_keys.len() {
            let mut e = k;
            while e < pair_keys.len() && pair_keys[e] == pair_keys[k] {
                e += 1;
            }
            let (a, b) = ((pair_keys[k] >> 32) as u32, pair_keys[k] as u32);
            pair_calls.insert((a, b), (e - k) as u64);
            weights.push((a, b, (e - k) as f64));
            k = e;
        }
    }
    drop(pair_keys);
    // Imports: the largest share per direction, both directions added.
    let mut imp: FxHashMap<(u32, u32), f32> = FxHashMap::default();
    let mut import_adj: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
    for (i, fo) in outs.iter().enumerate() {
        for &(line, j, share) in &fo.imports {
            let s = imp.entry((i as u32, j)).or_insert(0.0);
            *s = s.max(share);
            import_adj[i].push((j, line));
        }
    }
    let mut imp_pairs: FxHashMap<(u32, u32), f64> = FxHashMap::default();
    for (&(i, j), &s) in &imp {
        let key = if i < j { (i, j) } else { (j, i) };
        *imp_pairs.entry(key).or_default() += s as f64;
    }
    let n_imports = imp_pairs.len() as u64;
    let mut imp_sorted: Vec<((u32, u32), f64)> = imp_pairs.into_iter().collect();
    imp_sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for ((a, b), s) in imp_sorted {
        weights.push((a, b, IMPORT_WEIGHT * s));
    }
    drop(imp);
    let mut degree = vec![0u32; n];
    {
        let mut seen: FxHashSet<(u32, u32)> = FxHashSet::default();
        for &(a, b, _) in &weights {
            if seen.insert((a, b)) {
                degree[a as usize] += 1;
                degree[b as usize] += 1;
            }
        }
    }
    let mut g_edges = weights.clone();
    let g = Graph::new(n, &mut g_edges, vec![0.0; n]);
    drop(g_edges);
    lap("file graph", &mut timing);

    // ----- communities -----
    let is_isolated = |i: usize| g.off[i] == g.off[i + 1];
    let isolated: Vec<usize> = (0..n).filter(|&i| is_isolated(i)).collect();
    // Hub files are held out of the first partition and join a community afterwards (a
    // stabiliser of the study). A file that half the repository calls into (a string library,
    // an allocator, a logging package) says nothing about where its callers belong, and left in
    // it glues unrelated parts into a few giant communities. Files whose neighbours are all hubs
    // are held out with them, since without the hubs they have no edge.
    let (hub_file, stranded) = held_out(&g, &degree);
    let core: Vec<(u32, u32, f64)> = weights.iter().copied().filter(|e| !hub_file[e.0 as usize] && !hub_file[e.1 as usize]).collect();
    drop(weights);
    let first = louvain(n, &core);
    drop(core);
    const NO_C: u32 = u32::MAX;
    let mut comm: Vec<u32> = (0..n).map(|i| if hub_file[i] || stranded[i] || is_isolated(i) { NO_C } else { first[i] }).collect();
    let mut next_c = first.iter().copied().max().map(|x| x + 1).unwrap_or(0);
    // Each held file joins the community it has most weight to: hubs first, the most connected
    // first, then the files that hang on hubs, each seeing the joins before it. A file none of
    // whose neighbours has a community yet waits for the next round.
    let mut pending: Vec<usize> = (0..n).filter(|&i| hub_file[i] || stranded[i]).collect();
    pending.sort_by_key(|&i| (!hub_file[i], std::cmp::Reverse(degree[i]), i));
    loop {
        let mut rest = Vec::new();
        for &i in &pending {
            match heaviest_community(&g, &comm, i, NO_C) {
                Some(c) => comm[i] = c,
                None => rest.push(i),
            }
        }
        if rest.is_empty() || rest.len() == pending.len() {
            for &i in &rest {
                comm[i] = next_c;
                next_c += 1;
            }
            break;
        }
        pending = rest;
    }
    let mut groups: Vec<Vec<u32>> = {
        let mut by: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
        for i in 0..n {
            if comm[i] != NO_C {
                by.entry(comm[i]).or_default().push(i as u32);
            }
        }
        let mut v: Vec<Vec<u32>> = by.into_values().collect();
        v.sort_by_key(|m| m[0]);
        v
    };
    // A community larger than a quarter of the files (and than 10) is split by the same
    // procedure run inside it.
    let max_size = (n / 4).max(10);
    let mut split: Vec<Vec<u32>> = Vec::new();
    for m in groups.drain(..) {
        if m.len() <= max_size {
            split.push(m);
            continue;
        }
        let local: FxHashMap<u32, u32> = m.iter().enumerate().map(|(k, &x)| (x, k as u32)).collect();
        let mut sub: Vec<(u32, u32, f64)> = Vec::new();
        for (k, &x) in m.iter().enumerate() {
            for e in g.off[x as usize]..g.off[x as usize + 1] {
                if let Some(&l) = local.get(&g.nb[e]) {
                    if (k as u32) < l {
                        sub.push((k as u32, l, g.w[e]));
                    }
                }
            }
        }
        let parts = louvain(m.len(), &sub);
        let mut by: Vec<Vec<u32>> = Vec::new();
        for (k, &p) in parts.iter().enumerate() {
            if by.len() <= p as usize {
                by.resize(p as usize + 1, Vec::new());
            }
            by[p as usize].push(m[k]);
        }
        split.extend(by);
    }
    for (c, m) in split.iter().enumerate() {
        for &x in m {
            comm[x as usize] = c as u32;
        }
    }
    // A community of one or two files joins the neighbouring community it has most weight to.
    // The smallest go first, in path order, and each sees the joins before it.
    let mut order: Vec<usize> = (0..split.len()).filter(|&c| split[c].len() <= 2).collect();
    order.sort_by_key(|&c| (split[c].len(), split[c][0]));
    for c in order {
        if split[c].is_empty() || split[c].len() > 2 {
            continue;
        }
        let mut to: FxHashMap<u32, f64> = FxHashMap::default();
        for &x in &split[c] {
            for e in g.off[x as usize]..g.off[x as usize + 1] {
                let d = comm[g.nb[e] as usize];
                if d != c as u32 {
                    *to.entry(d).or_default() += g.w[e];
                }
            }
        }
        let mut cands: Vec<(u32, f64)> = to.into_iter().collect();
        cands.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        if let Some(&(d, _)) = cands.first() {
            let moved = std::mem::take(&mut split[c]);
            for &x in &moved {
                comm[x as usize] = d;
            }
            split[d as usize].extend(moved);
        }
    }
    // Final numbering: the largest first, then by first path.
    let mut members: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    for i in 0..n {
        if !is_isolated(i) {
            members.entry(comm[i]).or_default().push(i as u32);
        }
    }
    let mut final_groups: Vec<Vec<u32>> = members.into_values().collect();
    final_groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    let mut cid = vec![usize::MAX; n];
    for (c, m) in final_groups.iter().enumerate() {
        for &x in m {
            cid[x as usize] = c;
        }
    }
    lap("communities", &mut timing);

    // ----- most used definitions -----
    let mut ranked: Vec<(DefId, u64, u32)> = used.iter().map(|(&d, &(c, f))| (d, c, f)).collect();
    let path_line = |d: DefId| {
        let v = ix.def(d);
        (v.path(), v.rec.line)
    };
    ranked.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then_with(|| path_line(a.0).cmp(&path_line(b.0))).then(a.0.cmp(&b.0)));
    let eligible = |d: DefId| {
        let v = ix.def(d);
        !v.is_decl() && !trivial_accessor(ix, d)
    };
    let used_of = |d: DefId, calls: u64, files: u32| {
        let v = ix.def(d);
        Used { name: v.qualified(), path: v.path().to_string(), line: v.rec.line, calls, files, community: cid[node_of_def(d) as usize] }
    };
    let hub_n = ((ranked.len() as f64 * HUB_SHARE).ceil() as usize).max(HUBS_MAX);
    let mut hub_set: FxHashSet<DefId> = FxHashSet::default();
    let mut hubs: Vec<Used> = Vec::new();
    let mut top: Vec<Vec<Used>> = vec![Vec::new(); final_groups.len()];
    let mut filled = 0usize;
    for &(d, calls, files) in &ranked {
        let c = cid[node_of_def(d) as usize];
        let wanted = hub_set.len() < hub_n || (c != usize::MAX && top[c].len() < TOP_MAX);
        if !wanted {
            if filled == top.len() {
                break;
            }
            continue;
        }
        if !eligible(d) {
            continue;
        }
        if hub_set.len() < hub_n {
            hub_set.insert(d);
            if hubs.len() < HUBS_MAX {
                hubs.push(used_of(d, calls, files));
            }
        }
        if c != usize::MAX && top[c].len() < TOP_MAX {
            top[c].push(used_of(d, calls, files));
            if top[c].len() == TOP_MAX {
                filled += 1;
            }
        }
    }
    lap("most used", &mut timing);

    // ----- calls between communities, couplings -----
    let nc = final_groups.len();
    let mut into: FxHashMap<(usize, usize), u64> = FxHashMap::default();
    for (i, fo) in outs.iter().enumerate() {
        let ci = cid[i];
        for &(_, t) in &fo.calls {
            let cj = cid[node_of_def(t) as usize];
            if ci != cj && ci != usize::MAX && cj != usize::MAX {
                *into.entry((ci, cj)).or_default() += 1;
            }
        }
    }
    let between = |a: usize, b: usize| into.get(&(a, b)).copied().unwrap_or(0) + into.get(&(b, a)).copied().unwrap_or(0);
    // The score of a call from community A to community B, written so that each factor the
    // overview promises raises it:
    //
    //   score = (1 / calls between A and B, both ways)
    //         * (2 when the two files lie in different top-level directories, else 1)
    //         * (1.5 when the calling file is small, linked to at most two other files, and the
    //            called definition is a hub of the repository, else 1)
    //
    // Rarity comes first: a pair of communities with many calls between them is a known
    // dependency, whatever directories it crosses. The two bonuses order the rare ones. One
    // call per pair of communities is kept, the best scored (ties: the first by caller path,
    // line, callee path, line).
    //
    // A pair with more than `FEW_CALLS` calls that carries over a quarter of the calls either
    // community exchanges with all others is one of its main dependencies: expected, and not
    // reported. A handful of calls is rare enough to report whatever else the two exchange.
    let mut cross = vec![0u64; nc];
    for (&(a, b), &x) in &into {
        cross[a] += x;
        cross[b] += x;
    }
    let mut best_pair: FxHashMap<(usize, usize), Coupling> = FxHashMap::default();
    for (i, fo) in outs.iter().enumerate() {
        let ci = cid[i];
        if ci == usize::MAX {
            continue;
        }
        for &(a, t) in &fo.calls {
            let j = node_of_def(t) as usize;
            let cj = cid[j];
            if cj == ci || cj == usize::MAX {
                continue;
            }
            let n_between = between(ci, cj);
            if n_between > FEW_CALLS && (4 * n_between > cross[ci] || 4 * n_between > cross[cj]) {
                continue;
            }
            let (va, vt) = (ix.def(a), ix.def(t));
            let cross_dir = top_dir(va.path()) != top_dir(vt.path());
            let small_to_hub = degree[i] <= 2 && hub_set.contains(&t);
            let score = (1.0 / n_between as f64) * if cross_dir { 2.0 } else { 1.0 } * if small_to_hub { 1.5 } else { 1.0 };
            let key = (ci.min(cj), ci.max(cj));
            let cand = Coupling {
                from: ci,
                to: cj,
                caller: Endpoint { name: va.qualified(), path: va.path().to_string(), line: va.rec.line },
                callee: Endpoint { name: vt.qualified(), path: vt.path().to_string(), line: vt.rec.line },
                between: n_between,
                cross_dir,
                small_to_hub,
                score,
            };
            let better = match best_pair.get(&key) {
                None => true,
                Some(old) => cand.score > old.score || (cand.score == old.score && coupling_key(&cand) < coupling_key(old)),
            };
            if better {
                best_pair.insert(key, cand);
            }
        }
    }
    let mut couplings: Vec<Coupling> = best_pair.into_values().collect();
    couplings.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| coupling_key(a).cmp(&coupling_key(b))));
    couplings.truncate(COUPLINGS_MAX);
    let mut links: Vec<(usize, usize, u64)> = Vec::new();
    {
        let mut pairs: Vec<(usize, usize)> = into.keys().map(|&(a, b)| (a.min(b), a.max(b))).collect();
        pairs.sort_unstable();
        pairs.dedup();
        for (a, b) in pairs {
            links.push((a, b, between(a, b)));
        }
    }
    let mut calls_into: Vec<Vec<(usize, u64)>> = vec![Vec::new(); nc];
    for (&(a, b), &x) in &into {
        calls_into[a].push((b, x));
    }
    for v in &mut calls_into {
        v.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    }
    lap("couplings", &mut timing);

    // ----- import cycles -----
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut line_of: FxHashMap<(u32, u32), u32> = FxHashMap::default();
    for (i, list) in import_adj.iter().enumerate() {
        for &(j, line) in list {
            let e = line_of.entry((i as u32, j)).or_insert(line);
            *e = (*e).min(line);
            adj[i].push(j);
        }
        adj[i].sort_unstable();
        adj[i].dedup();
    }
    drop(import_adj);
    let mut comps = strong_components(&adj);
    let n_cycles = comps.len();
    comps.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    let mut cycles: Vec<Cycle> = Vec::new();
    for comp in comps.iter().take(CYCLES_MAX) {
        let (path, partial) = shortest_cycle(&adj, comp, CYCLE_STARTS);
        let steps = path
            .iter()
            .enumerate()
            .map(|(k, &x)| {
                let nx = path[(k + 1) % path.len()];
                (ix.path(nodes[x as usize]).to_string(), line_of.get(&(x, nx)).copied().unwrap_or(0))
            })
            .collect();
        cycles.push(Cycle { files: comp.len(), steps, partial });
    }
    lap("import cycles", &mut timing);

    // ----- names -----
    let mut communities: Vec<Community> = Vec::with_capacity(nc);
    for (c, m) in final_groups.iter().enumerate() {
        let files: Vec<String> = m.iter().map(|&x| ix.path(nodes[x as usize]).to_string()).collect();
        let defs = m.iter().map(|&x| outs[x as usize].defs as usize).sum();
        communities.push(Community { name: String::new(), files, defs, top: std::mem::take(&mut top[c]), calls_into: std::mem::take(&mut calls_into[c]) });
    }
    name_communities(&mut communities);
    lap("names", &mut timing);

    let isolated_paths: Vec<String> = isolated.iter().map(|&x| ix.path(nodes[x]).to_string()).collect();
    timing.push(("total", t0.elapsed()));
    Map { files: n, calls: total_calls, imports: n_imports, communities, isolated: isolated_paths, hubs, couplings, n_cycles, cycles, links, timing }
}

fn coupling_key(c: &Coupling) -> (&str, u32, &str, u32) {
    (&c.caller.path, c.caller.line, &c.callee.path, c.callee.line)
}

/// Names from paths. A community is named by the longest directory prefix its files share.
/// Where other communities share that prefix, the largest keeps the plain prefix when it has at
/// least twice the files of the next. The others, and all of them when they are of similar
/// size, add the name of their most used definition (or, with none, of their first file). A
/// community whose files lie in several top-level directories is named after the directory
/// that holds at least half of them, as `mostly DIR`, and `.` with its most used definition
/// when none does.
fn name_communities(cs: &mut [Community]) {
    let prefixes: Vec<String> = cs
        .iter()
        .map(|c| {
            let paths: Vec<&str> = c.files.iter().map(|s| s.as_str()).collect();
            let p = common_dir(&paths);
            if !p.is_empty() {
                return p;
            }
            // Files from several top-level directories: the directory that holds at least half.
            match dir_holding(&paths, paths.len().div_ceil(2)) {
                m if m.is_empty() => ".".to_string(),
                m => format!("mostly {m}"),
            }
        })
        .collect();
    let mut by_prefix: FxHashMap<&str, Vec<usize>> = FxHashMap::default();
    for (i, p) in prefixes.iter().enumerate() {
        by_prefix.entry(p.as_str()).or_default().push(i);
    }
    let label = |c: &Community| -> String {
        match c.top.first() {
            Some(u) => u.name.clone(),
            None => c.files.first().map(|f| f.rsplit('/').next().unwrap_or(f).to_string()).unwrap_or_default(),
        }
    };
    let mut names: Vec<String> = prefixes.clone();
    for (i, c) in cs.iter().enumerate() {
        let same = &by_prefix[prefixes[i].as_str()];
        // `.` says nothing on its own.
        if same.len() == 1 && prefixes[i] != "." {
            continue;
        }
        // `same` is in community order: the largest first.
        let lead = same.len() > 1 && prefixes[i] != "." && same[0] == i && cs[same[0]].files.len() >= 2 * cs[same[1]].files.len();
        if !lead {
            names[i] = format!("{} ({})", prefixes[i], label(c));
        }
    }
    // Two that still share a name tell themselves apart by their first file.
    let mut seen: FxHashMap<String, usize> = FxHashMap::default();
    for n in &names {
        *seen.entry(n.clone()).or_default() += 1;
    }
    for (i, c) in cs.iter_mut().enumerate() {
        let mut name = names[i].clone();
        if seen[&name] > 1 {
            let first = c.files.first().map(|f| f.rsplit('/').next().unwrap_or(f)).unwrap_or("");
            name = format!("{name} {first}");
        }
        c.name = name;
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// `1 file`, `2,345 files`.
fn n_of(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

fn header(m: &Map) -> String {
    let mut s = format!("{} in {}", n_of(m.files, "file", "files"), n_of(m.communities.len(), "community", "communities"));
    if !m.isolated.is_empty() {
        s.push_str(&format!(" and {}", n_of(m.isolated.len(), "isolated file", "isolated files")));
    }
    s.push_str(&format!(
        ". {} and {} between files.",
        n_of(m.calls as usize, "resolved call", "resolved calls"),
        n_of(m.imports as usize, "import", "imports")
    ));
    s
}

fn community_lines(m: &Map, c: usize) -> Vec<String> {
    let cm = &m.communities[c];
    let mut v = vec![format!("{}. {}: {}, {}", c + 1, cm.name, n_of(cm.files.len(), "file", "files"), n_of(cm.defs, "definition", "definitions"))];
    for u in cm.top.iter().take(3) {
        v.push(format!("   {}  {}:{}  {}", u.name, u.path, u.line, usage(u)));
    }
    if !cm.calls_into.is_empty() {
        let parts: Vec<String> = cm.calls_into.iter().take(3).map(|&(d, x)| format!("{} {}", m.communities[d].name, count(x as usize))).collect();
        v.push(format!("   calls into: {}", parts.join(", ")));
    }
    v
}

fn usage(u: &Used) -> String {
    format!("used by {}, {}", n_of(u.files as usize, "file", "files"), n_of(u.calls as usize, "call", "calls"))
}

fn isolated_line(m: &Map) -> Option<String> {
    if m.isolated.is_empty() {
        return None;
    }
    let ex: Vec<&str> = m.isolated.iter().take(3).map(|s| s.as_str()).collect();
    let more = if m.isolated.len() > 3 { ", …" } else { "" };
    Some(format!("isolated files: {} with no resolved call or import to or from another file ({}{more})", count(m.isolated.len()), ex.join(", ")))
}

fn hub_line(m: &Map, u: &Used) -> String {
    let c = m.communities.get(u.community).map(|c| c.name.as_str()).unwrap_or("isolated");
    format!("  {}  {}:{}  {}  [{}]", u.name, u.path, u.line, usage(u), c)
}

fn coupling_line(m: &Map, k: &Coupling) -> String {
    let mut why =
        vec![if k.between == 1 { "the only call between the two".to_string() } else { format!("{} calls between the two", count(k.between as usize)) }];
    if k.cross_dir {
        why.push("across top-level directories".to_string());
    }
    if k.small_to_hub {
        why.push("a small file reaching a hub".to_string());
    }
    format!(
        "  {} -> {}: {} {}:{} calls {} {}:{} ({})",
        m.communities[k.from].name,
        m.communities[k.to].name,
        k.caller.name,
        k.caller.path,
        k.caller.line,
        k.callee.name,
        k.callee.path,
        k.callee.line,
        why.join(", ")
    )
}

fn cycle_line(c: &Cycle) -> String {
    let mut s = format!("  {}", n_of(c.files, "file", "files"));
    if c.files > c.steps.len() {
        s.push_str(&format!(", shortest circle {}{}", c.steps.len(), if c.partial { " found" } else { "" }));
    }
    s.push_str(": ");
    let steps: Vec<String> = c.steps.iter().map(|(p, l)| format!("{p}:{l}")).collect();
    s.push_str(&steps.join(" -> "));
    if let Some((p, _)) = c.steps.first() {
        s.push_str(&format!(" -> {p}"));
    }
    s
}

const HUBS_TITLE: &str = "Hubs, by the files that use them:";
const COUPLINGS_TITLE: &str = "Unexpected couplings, the rarest first:";
const COUPLINGS_NONE: &str = "Unexpected couplings: none.";
const CYCLES_NONE: &str = "Import cycles: none.";

fn cycles_title(m: &Map) -> String {
    let of = if m.n_cycles > m.cycles.len() { format!(", the {} largest", m.cycles.len()) } else { String::new() };
    let groups = if m.n_cycles == 1 {
        "1 group of files that import each other".to_string()
    } else {
        format!("{} groups of files that import each other", count(m.n_cycles))
    };
    format!("Import cycles: {groups}{of}:")
}

fn cost(lines: &[String]) -> usize {
    lines.iter().map(|l| est_tokens(l) + 1).sum()
}

/// What fits in the budget: how many communities, hubs, couplings and cycles are shown.
struct Shown {
    communities: usize,
    hubs: usize,
    couplings: usize,
    cycles: usize,
}

/// Room kept for the line that says what was left out.
const FOOTER_ROOM: usize = 40;

/// Decide what is shown. Hubs, couplings and cycles are each kept up to 15% of the budget
/// (less when they need less), the communities get the rest, whole or not at all, and room the
/// communities leave goes to the three sections in order. So every section shows on the
/// largest repository, and nothing is printed past the budget.
fn select(m: &Map, budget: usize) -> Shown {
    if budget == 0 {
        return Shown { communities: m.communities.len(), hubs: m.hubs.len(), couplings: m.couplings.len(), cycles: m.cycles.len() };
    }
    let one = |l: &str| est_tokens(l) + 1;
    // Lines printed whatever the budget.
    let mut used = one(&header(m)) + FOOTER_ROOM + isolated_line(m).map(|l| one(&l)).unwrap_or(0);
    if m.couplings.is_empty() {
        used += one(COUPLINGS_NONE);
    }
    if m.n_cycles == 0 {
        used += one(CYCLES_NONE);
    }
    // Each section: its title and its lines.
    let sections: [(Option<String>, Vec<String>); 3] = [
        (Some(HUBS_TITLE.to_string()), m.hubs.iter().map(|u| hub_line(m, u)).collect()),
        (Some(COUPLINGS_TITLE.to_string()), m.couplings.iter().map(|k| coupling_line(m, k)).collect()),
        (Some(cycles_title(m)), m.cycles.iter().map(cycle_line).collect()),
    ];
    let share = budget * 15 / 100;
    let keep: Vec<usize> =
        sections.iter().map(|(t, lines)| if lines.is_empty() { 0 } else { (t.as_deref().map(one).unwrap_or(0) + cost(lines)).min(share) }).collect();
    let mut communities = 0;
    let kept: usize = keep.iter().sum();
    for c in 0..m.communities.len() {
        let x = cost(&community_lines(m, c));
        if used + x + kept > budget {
            break;
        }
        used += x;
        communities += 1;
    }
    let mut shown = [0usize; 3];
    for (k, (title, lines)) in sections.iter().enumerate() {
        if lines.is_empty() {
            continue;
        }
        let later: usize = keep[k + 1..].iter().sum();
        let title_cost = title.as_deref().map(one).unwrap_or(0);
        if used + title_cost + later > budget {
            continue;
        }
        let mut n = 0;
        let mut u = used + title_cost;
        for l in lines {
            if u + one(l) + later > budget {
                break;
            }
            u += one(l);
            n += 1;
        }
        if n > 0 {
            used = u;
            shown[k] = n;
        }
    }
    Shown { communities, hubs: shown[0], couplings: shown[1], cycles: shown[2] }
}

fn left_out(m: &Map, s: &Shown) -> Vec<String> {
    let mut v = Vec::new();
    let pairs = [
        (m.communities.len() - s.communities, "community", "communities"),
        (m.hubs.len() - s.hubs, "hub", "hubs"),
        (m.couplings.len() - s.couplings, "coupling", "couplings"),
        (m.cycles.len() - s.cycles, "import cycle", "import cycles"),
    ];
    for (n, one, many) in pairs {
        if n > 0 {
            v.push(format!("{} more {}", count(n), if n == 1 { one } else { many }));
        }
    }
    v
}

pub fn render_text(m: &Map, out: &mut Out) {
    let s = select(m, out.budget());
    out.push(&header(m));
    if m.files == 0 {
        return;
    }
    for c in 0..s.communities {
        for l in community_lines(m, c) {
            out.push(&l);
        }
    }
    if let Some(l) = isolated_line(m) {
        out.push(&l);
    }
    if s.hubs > 0 {
        out.push(HUBS_TITLE);
        for u in m.hubs.iter().take(s.hubs) {
            out.push(&hub_line(m, u));
        }
    }
    if m.couplings.is_empty() {
        out.push(COUPLINGS_NONE);
    } else if s.couplings > 0 {
        out.push(COUPLINGS_TITLE);
        for k in m.couplings.iter().take(s.couplings) {
            out.push(&coupling_line(m, k));
        }
    }
    if m.n_cycles == 0 {
        out.push(CYCLES_NONE);
    } else if s.cycles > 0 {
        out.push(&cycles_title(m));
        for c in m.cycles.iter().take(s.cycles) {
            out.push(&cycle_line(c));
        }
    }
    let lo = left_out(m, &s);
    if !lo.is_empty() {
        out.push(&format!("… {} (raise --budget, or -p DIR for one part)", lo.join(", ")));
    }
}

fn used_json(u: &Used) -> Value {
    json!({"name": u.name, "path": u.path, "line": u.line, "calls": u.calls, "files": u.files, "community": u.community + 1})
}

fn endpoint_json(e: &Endpoint) -> Value {
    json!({"name": e.name, "path": e.path, "line": e.line})
}

/// The text overview as JSON: the same selection, with the left-out counts.
pub fn to_json(m: &Map, budget: usize) -> Value {
    let s = select(m, budget);
    let communities: Vec<Value> = m
        .communities
        .iter()
        .enumerate()
        .take(s.communities)
        .map(|(i, c)| {
            json!({
                "id": i + 1,
                "name": c.name,
                "files": c.files.len(),
                "definitions": c.defs,
                "top": c.top.iter().take(3).map(used_json).collect::<Vec<_>>(),
                "calls_into": c.calls_into.iter().take(3).map(|&(d, x)| json!({"community": d + 1, "name": m.communities[d].name, "calls": x})).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "files": m.files,
        "calls": m.calls,
        "imports": m.imports,
        "communities": communities,
        "isolated": {"files": m.isolated.len(), "examples": m.isolated.iter().take(3).collect::<Vec<_>>()},
        "hubs": m.hubs.iter().take(s.hubs).map(used_json).collect::<Vec<_>>(),
        "couplings": m.couplings.iter().take(s.couplings).map(|k| json!({
            "from": k.from + 1, "to": k.to + 1, "caller": endpoint_json(&k.caller), "callee": endpoint_json(&k.callee),
            "calls_between": k.between, "cross_directory": k.cross_dir, "small_file_to_hub": k.small_to_hub, "score": k.score,
        })).collect::<Vec<_>>(),
        "cycles": m.cycles.iter().take(s.cycles).map(|c| json!({
            "files": c.files,
            "steps": c.steps.iter().map(|(p, l)| json!({"path": p, "line": l})).collect::<Vec<_>>(),
            "partial": c.partial,
        })).collect::<Vec<_>>(),
        "cycles_total": m.n_cycles,
        "left_out": {
            "communities": m.communities.len() - s.communities,
            "hubs": m.hubs.len() - s.hubs,
            "couplings": m.couplings.len() - s.couplings,
            "cycles": m.cycles.len() - s.cycles,
        },
    })
}

const PAGE: &str = include_str!("map.html");

/// The HTML page: the template with the data embedded as JSON. Nothing in it loads from the
/// network.
pub fn render_html(m: &Map, title: &str) -> String {
    let drawn = m.communities.len().min(DRAW_MAX);
    let mut links: Vec<&(usize, usize, u64)> = m.links.iter().filter(|l| l.0 < drawn && l.1 < drawn).collect();
    let links_total = links.len();
    links.sort_by(|a, b| b.2.cmp(&a.2).then((a.0, a.1).cmp(&(b.0, b.1))));
    links.truncate(LINKS_MAX);
    links.sort_by_key(|l| (l.0, l.1));
    let communities: Vec<Value> = m
        .communities
        .iter()
        .take(drawn)
        .enumerate()
        .map(|(i, c)| {
            // The colour: the top-level directory that holds most of its files, and the
            // directory two levels down for a repository that keeps everything in one.
            let dir = majority(c.files.iter().map(|p| top_dir(p)));
            let dir2 = majority(c.files.iter().map(|p| top_two(p)));
            json!({
                "id": i + 1,
                "name": c.name,
                "dir": dir,
                "dir2": dir2,
                "files": c.files.len(),
                "definitions": c.defs,
                "paths": c.files.iter().take(FILES_MAX).collect::<Vec<_>>(),
                "top": c.top.iter().map(used_json).collect::<Vec<_>>(),
                "calls_into": c.calls_into.iter().take(8).map(|&(d, x)| json!({"community": d + 1, "name": m.communities[d].name, "calls": x})).collect::<Vec<_>>(),
            })
        })
        .collect();
    let data = json!({
        "title": title,
        "files": m.files,
        "calls": m.calls,
        "imports": m.imports,
        "communities_total": m.communities.len(),
        "isolated": m.isolated.len(),
        "communities": communities,
        "links": links.iter().map(|l| json!([l.0 + 1, l.1 + 1, l.2])).collect::<Vec<_>>(),
        "links_total": links_total,
        "hubs": m.hubs.iter().map(used_json).collect::<Vec<_>>(),
        "couplings": m.couplings.iter().map(|k| json!({
            "from": k.from + 1, "to": k.to + 1, "caller": endpoint_json(&k.caller), "callee": endpoint_json(&k.callee), "calls_between": k.between,
        })).collect::<Vec<_>>(),
        "cycles": m.cycles.iter().map(|c| json!({
            "files": c.files,
            "steps": c.steps.iter().map(|(p, l)| json!({"path": p, "line": l})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "cycles_total": m.n_cycles,
    });
    // Inside a script element `</` would end it: JSON allows `<\/` for the same text.
    let text = data.to_string().replace("</", "<\\/").replace("<!--", "<\\u0021--");
    PAGE.replace("/*MAP_DATA*/null", &text).replace("{{TITLE}}", &html_escape(title))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Timing for `--verbose`.
pub fn timing_line(m: &Map) -> String {
    let parts: Vec<String> = m.timing.iter().map(|(k, d)| format!("{k} {:.2?}", d)).collect();
    format!("map: {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two cliques of four joined by one light edge, and one triangle on its own.
    #[test]
    fn louvain_finds_known_communities() {
        let mut e: Vec<(u32, u32, f64)> = Vec::new();
        for group in [[0u32, 1, 2, 3], [4, 5, 6, 7]] {
            for a in 0..4 {
                for b in a + 1..4 {
                    e.push((group[a], group[b], 1.0));
                }
            }
        }
        e.push((3, 4, 0.5));
        e.extend([(8, 9, 2.0), (9, 10, 2.0), (8, 10, 2.0)]);
        let c = louvain(12, &e);
        assert_eq!(c[0..4], [c[0]; 4]);
        assert_eq!(c[4..8], [c[4]; 4]);
        assert_eq!(c[8..11], [c[8]; 3]);
        assert_ne!(c[0], c[4]);
        assert_ne!(c[0], c[8]);
        assert_ne!(c[4], c[8]);
        // Node 11 has no edge: alone.
        assert!(c[..11].iter().all(|&x| x != c[11]));
        // Numbered by first node, and the same every time, whatever order the edges came in.
        assert_eq!(c[0], 0);
        let mut rev = e.clone();
        rev.reverse();
        assert_eq!(louvain(12, &rev), c);
    }

    /// A ring of five cliques of five, each joined to the next by one edge: the classic case
    /// where modularity puts each clique in its own community.
    #[test]
    fn louvain_ring_of_cliques() {
        let mut e: Vec<(u32, u32, f64)> = Vec::new();
        for k in 0..5u32 {
            let base = k * 5;
            for a in 0..5 {
                for b in a + 1..5 {
                    e.push((base + a, base + b, 1.0));
                }
            }
            e.push((base + 4, (base + 5) % 25, 1.0));
        }
        let c = louvain(25, &e);
        for k in 0..5 {
            let s = &c[k * 5..k * 5 + 5];
            assert!(s.iter().all(|&x| x == s[0]), "clique {k}: {c:?}");
        }
        let distinct: FxHashSet<u32> = c.iter().copied().collect();
        assert_eq!(distinct.len(), 5);
    }

    #[test]
    fn cycles_are_found_with_their_shortest_circle() {
        // 0 -> 1 -> 2 -> 0 and 1 -> 0 (shortest: 0 1), 3 -> 4 -> 3, 5 alone, 6 -> 5.
        let adj: Vec<Vec<u32>> = vec![vec![1], vec![0, 2], vec![0], vec![4], vec![3], vec![], vec![5]];
        let comps = strong_components(&adj);
        assert_eq!(comps, vec![vec![0, 1, 2], vec![3, 4]]);
        assert_eq!(shortest_cycle(&adj, &comps[0], 100), (vec![0, 1], false));
        assert_eq!(shortest_cycle(&adj, &comps[1], 100), (vec![3, 4], false));
        // A circle of four with no shortcut.
        let ring: Vec<Vec<u32>> = vec![vec![1], vec![2], vec![3], vec![0]];
        let comps = strong_components(&ring);
        assert_eq!(comps, vec![vec![0, 1, 2, 3]]);
        assert_eq!(shortest_cycle(&ring, &comps[0], 100).0, vec![0, 1, 2, 3]);
        // No cycle at all.
        assert!(strong_components(&[vec![1], vec![2], vec![]]).is_empty());
    }

    #[test]
    fn common_directory() {
        assert_eq!(common_dir(&["src/a.rs", "src/ab/x.rs"]), "src");
        assert_eq!(common_dir(&["src/q/a.rs", "src/q/b.rs"]), "src/q");
        assert_eq!(common_dir(&["a.rs", "src/b.rs"]), "");
        assert_eq!(common_dir(&["src/only.rs"]), "src/only.rs");
        // One stray file in ten does not take the name away.
        let mut many: Vec<String> = (0..10).map(|i| format!("src/core/f{i}.rs")).collect();
        many.push("examples/x.rs".to_string());
        let refs: Vec<&str> = many.iter().map(|s| s.as_str()).collect();
        assert_eq!(common_dir(&refs), "src/core");
    }
}
