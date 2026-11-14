use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use codefast::facts::DefKind;
use codefast::output::Out;
use codefast::query::{self, Common};
use codefast::resolve::Resolver;
use std::path::PathBuf;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "codefast", version, about = "Code intelligence for AI coding agents: exact answers about a repository in few tokens.")]
struct Cli {
    /// Repository root (default: nearest ancestor with .codefast or .git, else the current directory)
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Token budget for the answer (0 = unlimited)
    #[arg(long, global = true, default_value_t = 1500)]
    budget: usize,
    /// Restrict to paths (substring, or glob when it contains *); repeatable
    #[arg(long = "path", short = 'p', global = true)]
    paths: Vec<String>,
    /// Restrict to symbol kinds: fn, type, struct, class, macro, var, const, field, ... (comma separated)
    #[arg(long, short = 'k', global = true)]
    kind: Option<String>,
    /// Include test code in results
    #[arg(long, global = true)]
    tests: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start here: a symbol name, a path, or free text (bug report, question, stack trace)
    Search(SearchArgs),
    /// Find symbols by name (exact, prefix, substring, words in any order, Owner.name, globs)
    Find(FindArgs),
    /// Show source: a symbol's body, a file's outline, `path:line`, or `path:start-end`
    Show(ShowArgs),
    /// References to a symbol, grouped by kind and file
    Refs(RefsArgs),
    /// Who calls a function, including through callback slots; --depth N for a caller tree
    Callers(CallersArgs),
    /// What a function calls, with where each callee is defined
    Callees(CalleesArgs),
    /// Before changing a function: its body, its types, what it calls, who calls it, its tests
    #[command(alias = "ctx")]
    Context(ContextArgs),
    /// After an edit: uses left behind of what the uncommitted changes removed, renamed or re-signatured
    Check(CheckArgs),
    /// The uncommitted changes by definition (added, removed, altered), then what `check` finds
    Changed,
    /// Rename a function, method, type or field: its definition and every reference the index is sure of
    Rename(RenameArgs),
    /// Shortest call chain between two functions
    Path(PathArgs),
    /// Regex search over all text, with the enclosing symbol of every match
    Grep(GrepArgs),
    /// Rank files and symbols for free text: a bug report, a question, a stack trace
    Locate(LocateArgs),
    /// Resolve a pasted stack trace to code: one frame per line with location and signature
    Trace(TraceArgs),
    /// Look inside the project's dependencies: list them, or search one for a symbol or text
    Dep(DepArgs),
    /// Build or rebuild the index
    Index(IndexArgs),
    /// Index statistics
    Status,
    /// Serve the Model Context Protocol on stdio (for harnesses that prefer MCP to the CLI)
    Mcp,
    /// Register codefast with coding harnesses (Claude Code, Codex, Gemini CLI, Cursor, ...)
    #[command(disable_help_flag = true)]
    Setup {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Check the installation and every harness registration
    #[command(disable_help_flag = true)]
    Doctor {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Harness hook entry point (reads the hook payload on stdin)
    #[command(hide = true, disable_help_flag = true)]
    Hook {
        event: Option<String>,
        /// Anything a harness appends is accepted and ignored: a hook never fails on its
        /// arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    #[command(hide = true)]
    DebugFacts { file: PathBuf },
    /// Dump the resolved call graph as JSON Lines (for benchmarks)
    #[command(hide = true)]
    ExportEdges {
        /// Also emit dynamic-dispatch candidates
        #[arg(long)]
        dynamic: bool,
        /// Also emit what a call matches by name alone, one line per candidate (kind `name`)
        #[arg(long)]
        loose: bool,
    },
}

#[derive(Args)]
struct RenameArgs {
    /// What to rename: `name`, `Owner.name` or `path:line`
    target: String,
    /// The name it gets
    new_name: String,
    /// Show what would change and change nothing
    #[arg(long)]
    dry_run: bool,
    /// Rename a method although it may be part of an interface
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct SearchArgs {
    /// A symbol, `Owner.name`, a path, `path:line`, or any text; several queries are answered in turn
    #[arg(required = true)]
    queries: Vec<String>,
}

#[derive(Args)]
struct FindArgs {
    /// One or more queries
    #[arg(required = true)]
    queries: Vec<String>,
    /// Maximum results per query
    #[arg(long, short = 'n', default_value_t = 15)]
    limit: usize,
    /// Treat queries as regular expressions over symbol names
    #[arg(long, short = 'e')]
    regex: bool,
}

#[derive(Args)]
struct ShowArgs {
    /// Symbols, files, `path:line` or `path:start-end`
    #[arg(required = true)]
    targets: Vec<String>,
    /// Print bodies in full, however long
    #[arg(long)]
    full: bool,
    /// Signatures only
    #[arg(long, short = 'o')]
    outline: bool,
    /// Longest body printed without --full
    #[arg(long, default_value_t = 160)]
    max_lines: usize,
    /// Leave out leading doc comments
    #[arg(long)]
    no_doc: bool,
}

#[derive(Args)]
struct RefsArgs {
    #[arg(required = true)]
    targets: Vec<String>,
    /// Print the source line of every reference
    #[arg(long, short = 't')]
    text: bool,
    /// Names and line numbers only
    #[arg(long, conflicts_with = "text")]
    brief: bool,
    /// Keep only these reference kinds: call, read, write, type, bind (comma separated)
    #[arg(long)]
    only: Option<String>,
    /// Also list name-only matches whose receiver type is unknown
    #[arg(long)]
    loose: bool,
}

#[derive(Args)]
struct CallersArgs {
    #[arg(required = true)]
    targets: Vec<String>,
    /// Expand callers of callers up to this depth
    #[arg(long, short = 'd', default_value_t = 1)]
    depth: u32,
}

#[derive(Args)]
struct ContextArgs {
    #[arg(required = true)]
    targets: Vec<String>,
}

#[derive(Args)]
struct CheckArgs {
    /// Files to check (default: every file that differs from the last commit)
    files: Vec<String>,
    /// Treat all code as just written and count what the check for members that do not exist
    /// would report: in code that works, each report is a false alarm.
    #[arg(long, hide = true)]
    false_alarms: bool,
}

#[derive(Args)]
struct CalleesArgs {
    #[arg(required = true)]
    targets: Vec<String>,
    /// Include each callee's signature
    #[arg(long, short = 's')]
    sigs: bool,
}

#[derive(Args)]
struct PathArgs {
    from: String,
    to: String,
    #[arg(long, short = 'd', default_value_t = 12)]
    max_depth: u32,
}

#[derive(Args)]
struct GrepArgs {
    /// Regular expression (Rust regex syntax)
    pattern: String,
    /// Case-insensitive
    #[arg(short = 'i', long)]
    ignore_case: bool,
    /// Match whole words only
    #[arg(short = 'w', long)]
    word: bool,
    /// Treat the pattern as a literal string
    #[arg(short = 'F', long)]
    fixed: bool,
    /// List matching files with counts only
    #[arg(short = 'l', long)]
    files: bool,
    /// Lines of context after each match
    #[arg(short = 'A', long, default_value_t = 0)]
    after: u32,
}

#[derive(Args)]
struct LocateArgs {
    /// The text (several arguments are joined); use `-` to read it from stdin
    #[arg(required = true)]
    text: Vec<String>,
    /// Number of files to return
    #[arg(long, short = 'n', default_value_t = 10)]
    limit: usize,
    /// Symbols listed per file
    #[arg(long, default_value_t = 3)]
    symbols: usize,
    /// Machine-readable ranking (for scripts and benchmarks)
    #[arg(long)]
    plain: bool,
}

#[derive(Args)]
struct TraceArgs {
    /// File holding the trace, or `-` (the default) to read it from stdin
    #[arg(default_value = "-")]
    file: String,
}

#[derive(Args)]
struct DepArgs {
    /// Dependency name (omit to list the dependencies whose source is on disk)
    name: Option<String>,
    /// What to look up inside it: a symbol, a file, or text (omit for an overview)
    query: Vec<String>,
}

#[derive(Args)]
struct IndexArgs {
    /// Print timing and size details
    #[arg(long, short = 'v')]
    verbose: bool,
}

/// The repository a command is about: `--root` as given. Otherwise the nearest directory at or
/// above `CODEFAST_ROOT`, or the current directory, that is a git work tree or already holds an
/// index, and failing that the starting directory itself.
fn find_root(explicit: Option<PathBuf>) -> PathBuf {
    root_from(explicit, true)
}

/// `find_root`, with the choice of handing `CODEFAST_ROOT` on as it is written. The MCP server
/// resolves that directory itself, and tells the client when the repository starts above it.
fn root_from(explicit: Option<PathBuf>, resolve_env: bool) -> PathBuf {
    if let Some(r) = explicit {
        return r;
    }
    let start = match std::env::var_os("CODEFAST_ROOT").filter(|v| !v.is_empty()) {
        Some(r) if !resolve_env => return PathBuf::from(r),
        Some(r) => PathBuf::from(r),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let mut dir = start.as_path();
    loop {
        if dir.join(".codefast").is_dir() || dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return start,
        }
    }
}

/// Why `root` must not be indexed without being asked: it is the home directory or a directory
/// above it, and it is neither a repository nor already indexed. A query typed there by mistake
/// would otherwise walk everything the user owns.
fn home_refusal(root: &std::path::Path) -> Option<String> {
    let home = ["HOME", "USERPROFILE"].iter().filter_map(std::env::var_os).filter(|v| !v.is_empty()).find_map(|v| PathBuf::from(v).canonicalize().ok())?;
    let real = root.canonicalize().ok()?;
    if !home.starts_with(&real) || real.join(".git").exists() || codefast::build::index_dir(&real).join(codefast::build::BASE_SEGMENT).exists() {
        return None;
    }
    let what = if home == real { "the home directory" } else { "a directory above the home directory" };
    Some(format!(
        "{} is {what}, not a repository. Run codefast inside a project or pass --root DIR (to index this directory anyway, run `codefast index` here once)",
        real.display()
    ))
}

fn parse_kinds(s: &Option<String>) -> Result<Option<Vec<DefKind>>> {
    let Some(s) = s else { return Ok(None) };
    let mut out = Vec::new();
    for part in s.split(',').map(|p| p.trim()).filter(|p| !p.is_empty()) {
        match DefKind::parse(part) {
            Some(k) => out.extend(k),
            None => anyhow::bail!("unknown kind `{part}` (use: fn, method, type, struct, class, enum, iface, trait, macro, var, const, field, mod)"),
        }
    }
    Ok(Some(out))
}

/// Write to standard output. A reader that has gone away (`codefast .. | head`) is not an
/// error: the process ends quietly.
fn emit(s: &str) {
    use std::io::Write;
    let mut o = std::io::stdout().lock();
    if o.write_all(s.as_bytes()).and_then(|_| o.flush()).is_err() {
        std::process::exit(0);
    }
}

fn main() {
    let cli = Cli::parse();
    let res = if matches!(cli.cmd, Cmd::Dep(_)) { run_dep(cli) } else { run(cli) };
    if let Err(e) = res {
        let gone = e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe));
        if gone {
            std::process::exit(0);
        }
        eprintln!("codefast: {e:#}");
        std::process::exit(1);
    }
}

/// `codefast dep`: answer from the source of an installed dependency. The dependency is indexed
/// like a repository of its own. Its index is kept with the project's index, never inside the
/// package directory.
fn run_dep(cli: Cli) -> Result<()> {
    let Cmd::Dep(a) = &cli.cmd else { unreachable!() };
    let root = find_root(cli.root.clone());
    let root = root.canonicalize().unwrap_or(root);
    let deps = codefast::deps::discover(&root);
    let Some(name) = &a.name else {
        let mut out = Out::new(cli.budget);
        if deps.is_empty() {
            out.push("no dependency source found (looked for a Python virtual environment, node_modules, a Go vendor directory or module cache, and the cargo registry)");
        } else {
            out.push(&format!("{} dependencies with source on disk ({} direct):", deps.len(), deps.iter().filter(|d| d.direct).count()));
            for d in &deps {
                if !out.line(&format!("  {} {} {}{}", d.eco, d.name, d.version, if d.direct { "" } else { "  (indirect)" })) {
                    break;
                }
            }
            out.push("look inside one: codefast dep NAME [symbol or text]");
        }
        emit(&out.finish());
        return Ok(());
    };
    let found = codefast::deps::lookup(&deps, name).first().map(|d| (*d).clone()).or_else(|| codefast::deps::node_package(&root, name));
    let Some(dep) = found else {
        let near = codefast::deps::similar(&deps, name);
        let hint = if near.is_empty() { String::new() } else { format!(" (did you mean: {})", near.join(", ")) };
        anyhow::bail!("no dependency named `{name}` has its source on disk{hint}");
    };
    // A single-file module is looked up inside the directory that holds it.
    let single = dep.dir.is_file();
    let dep_root = if single { dep.dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| dep.dir.clone()) } else { dep.dir.clone() };
    let key = if single { format!("{}-site", dep.eco) } else { codefast::deps::index_key(&dep) };
    let idx = codefast::build::index_dir(&root).join("deps").join(key);
    std::fs::create_dir_all(&idx)?;
    // SAFETY: no other thread exists yet: the worker pool is created in `run`.
    unsafe { std::env::set_var("CODEFAST_INDEX_DIR", &idx) };
    emit(&format!("{} {} ({})  {}\n", dep.name, dep.version, dep.eco, dep.dir.display()));
    let mut paths = cli.paths.clone();
    if single {
        if let Some(f) = dep.dir.file_name() {
            paths.push(f.to_string_lossy().into_owned());
        }
    }
    let overview = a.query.is_empty();
    let cmd = if !overview {
        Cmd::Search(SearchArgs { queries: a.query.clone() })
    } else {
        let entries =
            if single { vec![dep.dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()] } else { codefast::deps::entry_files(&dep) };
        if entries.is_empty() {
            // No single entry file (a Go package is a directory): the exported names.
            Cmd::Find(FindArgs { queries: vec!["*".to_string()], limit: 40, regex: false })
        } else {
            Cmd::Show(ShowArgs { targets: entries, full: false, outline: true, max_lines: 160, no_doc: true })
        }
    };
    run(Cli { root: Some(dep_root), budget: cli.budget, paths, kind: cli.kind.clone(), tests: true, cmd })?;
    if overview {
        emit(&format!("look up a name or text inside it: codefast dep {} NAME\n", name));
    }
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    let root = root_from(cli.root.clone(), !matches!(cli.cmd, Cmd::Mcp));
    let asked =
        cli.root.is_some() || matches!(cli.cmd, Cmd::Index(_) | Cmd::DebugFacts { .. } | Cmd::Mcp | Cmd::Setup { .. } | Cmd::Doctor { .. } | Cmd::Hook { .. });
    if !asked {
        if let Some(why) = home_refusal(&root) {
            anyhow::bail!(why);
        }
    }
    codefast::build::init_thread_pool(thread_count(&cli.cmd));
    let common = Common { paths: cli.paths.clone(), kinds: parse_kinds(&cli.kind)?, tests: cli.tests, budget: cli.budget };

    match &cli.cmd {
        Cmd::Index(a) => {
            let root = root.canonicalize()?;
            let r = codefast::build::build_base(&root, &codefast::build::BuildOptions::default())?;
            println!(
                "indexed {} files ({} parsed, {} lines): {} symbols, {} references in {:.2?}",
                r.stats.files, r.stats.parsed_files, r.stats.lines, r.stats.defs, r.stats.refs, r.total
            );
            if a.verbose {
                println!("walk {:.2?}, extract {:.2?}, write {:.2?}", r.walk, r.extract, r.write);
                println!("names={} postings={} imports={} index={:.1}MB", r.stats.names, r.stats.postings, r.stats.imports, r.stats.bytes as f64 / 1e6);
                let seg = codefast::index::Segment::open(&codefast::build::index_dir(&root).join(codefast::build::BASE_SEGMENT))?;
                for (name, bytes) in seg.section_sizes() {
                    println!("  {name:9} {:8.1}MB", bytes as f64 / 1e6);
                }
            }
            return Ok(());
        }
        Cmd::DebugFacts { file } => {
            debug_facts(&file.to_string_lossy());
            return Ok(());
        }
        Cmd::Mcp => return codefast::mcp::serve(root),
        Cmd::Setup { args } => {
            emit(&codefast::setup::setup(&root, args)?);
            return Ok(());
        }
        Cmd::Doctor { args } => {
            emit(&codefast::setup::doctor(&root, args)?);
            return Ok(());
        }
        Cmd::Hook { event, .. } => {
            // A hook must never break the harness: any failure is silent, and a payload is
            // read up to a fixed size only.
            let Some(event) = event else { return Ok(()) };
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::Read::take(std::io::stdin(), (64 << 20) + 1), &mut input);
            if let Ok(out) = codefast::setup::hook(&root, event, &input) {
                emit(&out);
            }
            return Ok(());
        }
        _ => {}
    }

    let ix = codefast::fresh::open(&root, &codefast::fresh::OpenOptions::default())?;
    let rs = Resolver::new(&ix);
    // An edit context is several answers in one: its default budget is larger.
    let budget = if matches!(cli.cmd, Cmd::Context(_)) && cli.budget == 1500 { 3000 } else { cli.budget };
    let mut out = Out::new(budget);
    match &cli.cmd {
        Cmd::Search(a) => {
            for (i, q) in a.queries.iter().enumerate() {
                if i > 0 {
                    out.push("");
                }
                query::search::search(&ix, &rs, q, &common, &query::search::SearchOpts::default(), &mut out);
            }
        }
        Cmd::Find(a) => {
            let opts = query::find::FindOpts { limit: a.limit, regex: a.regex };
            query::find::find(&ix, &rs, &a.queries, &common, &opts, &mut out);
        }
        Cmd::Show(a) => {
            let opts = query::show::ShowOpts { max_lines: a.max_lines, full: a.full, outline: a.outline, doc: !a.no_doc };
            query::show::show(&ix, &rs, &a.targets, &common, &opts, &mut out);
        }
        Cmd::Refs(a) => {
            let text = if a.text {
                Some(true)
            } else if a.brief {
                Some(false)
            } else {
                None
            };
            let only = a.only.as_ref().map(|s| s.split(',').map(|x| x.trim().to_string()).collect()).unwrap_or_default();
            let opts = query::refs::RefsOpts { text, only, loose: a.loose };
            query::refs::refs(&ix, &rs, &a.targets, &common, &opts, &mut out);
        }
        Cmd::Callers(a) => query::refs::callers(&ix, &rs, &a.targets, &common, a.depth, &mut out),
        Cmd::Callees(a) => query::refs::callees(&ix, &rs, &a.targets, &common, a.sigs, &mut out),
        Cmd::Context(a) => query::context::context(&ix, &rs, &a.targets, &common, &mut out),
        Cmd::Check(a) if a.false_alarms => query::changed::false_alarms(&ix, &rs, &common, &mut out),
        Cmd::Check(a) => query::changed::check(&ix, &rs, &a.files, &common, &mut out),
        Cmd::Changed => query::changed::changed(&ix, &rs, &common, &mut out),
        Cmd::Rename(a) => query::rename::rename(&ix, &rs, &ix.root, &a.target, &a.new_name, &query::rename::Options { dry_run: a.dry_run, force: a.force }, &mut out),
        Cmd::Path(a) => query::refs::path(&ix, &rs, &a.from, &a.to, &common, a.max_depth, 8.0, &mut out),
        Cmd::Locate(a) => {
            let text = if a.text.len() == 1 && a.text[0] == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s
            } else {
                a.text.join(" ")
            };
            let opts = query::locate::LocateOpts { top_files: a.limit, syms_per_file: a.symbols, plain: a.plain, brief: false };
            query::locate::locate(&ix, &text, &common, &opts, &mut out);
        }
        Cmd::Trace(a) => {
            let text = if a.file == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s
            } else {
                std::fs::read_to_string(&a.file)?
            };
            query::trace::trace(&ix, &rs, &text, &common, &mut out);
        }
        Cmd::ExportEdges { dynamic, loose } => {
            let pf = query::PathFilter::new(&common.paths);
            let stdout = std::io::stdout();
            let mut w = std::io::BufWriter::new(stdout.lock());
            let n = query::export::export_edges(&ix, &rs, &pf, *dynamic, *loose, &mut w)?;
            eprintln!("{n} edges");
            return Ok(());
        }
        Cmd::Grep(a) => {
            let opts = query::grep::GrepOpts { ignore_case: a.ignore_case, word: a.word, fixed: a.fixed, files_only: a.files, after: a.after };
            query::grep::grep(&ix, &a.pattern, &common, &opts, &mut out);
        }
        Cmd::Status => {
            let seg = ix.segment(0);
            out.push(&format!("root {}", ix.root.display()));
            for key in ["files", "defs", "refs", "names", "lines", "tool"] {
                if let Some(v) = seg.meta(key) {
                    out.push(&format!("{key} {v}"));
                }
            }
            out.push(&format!("index {:.1}MB", seg.byte_len() as f64 / 1e6));
            if ix.overlay_files() > 0 {
                out.push(&format!("overlay {} files changed since the base index was built", ix.overlay_files()));
            }
        }
        Cmd::Index(_) | Cmd::DebugFacts { .. } | Cmd::Mcp | Cmd::Setup { .. } | Cmd::Doctor { .. } | Cmd::Hook { .. } | Cmd::Dep(_) => unreachable!(),
    }
    emit(&out.finish());
    Ok(())
}

/// How many threads to work with. `CODEFAST_THREADS` decides when it is set. Otherwise every
/// core, except for the refresh a hook starts in the background: nobody is waiting for that
/// one, so it leaves half of the machine to whatever else is running.
fn thread_count(cmd: &Cmd) -> usize {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    if let Some(n) = std::env::var("CODEFAST_THREADS").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|n| *n > 0) {
        return n.min(256);
    }
    match cmd {
        Cmd::Hook { event: Some(e), .. } if e == "index-refresh" => (cores / 2).max(1),
        _ => cores,
    }
}

fn debug_facts(path: &str) {
    let src = std::fs::read(path).expect("read");
    emit(&codefast::debug::dump_facts(path, &src));
}
