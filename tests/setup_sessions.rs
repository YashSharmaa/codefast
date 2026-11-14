//! `codefast doctor sessions`: where the tokens of a project's sessions went, from the
//! harness's own transcripts. The transcripts here are small synthetic ones written into a
//! sandboxed home, and the report reads them and changes nothing.

#[path = "setup_support/mod.rs"]
mod sandbox;

use sandbox::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const FIRST: &str = "5f0c2a4e-0000-4000-8000-000000000001";
const SECOND: &str = "9b7d1e3c-0000-4000-8000-000000000002";

/// The project lives below the home directory, as projects do.
fn project(sb: &Sandbox, name: &str) -> PathBuf {
    let root = sb.home(&format!("code/{name}"));
    write(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
    root
}

/// Where Claude Code keeps the transcripts of `root`.
fn transcripts(sb: &Sandbox, root: &Path) -> PathBuf {
    let slug: String = root.to_string_lossy().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    sb.home(".claude/projects").join(slug)
}

/// A time `minute` minutes after 09:00 on 10 November 2026.
fn ts(minute: i64) -> String {
    let m = 9 * 60 + minute;
    format!("2026-11-{:02}T{:02}:{:02}:00.000Z", 10 + m / 1440, m % 1440 / 60, m % 60)
}

fn usage(input: u64, read: u64, write: u64, output: u64, thinking: u64) -> Value {
    json!({
        "input_tokens": input,
        "cache_read_input_tokens": read,
        "cache_creation_input_tokens": write,
        "cache_creation": {"ephemeral_5m_input_tokens": write, "ephemeral_1h_input_tokens": 0},
        "output_tokens": output,
        "output_tokens_details": {"thinking_tokens": thinking},
        "service_tier": "standard",
    })
}

/// The usage of an older harness version: no thinking tokens and no split of the cache writes.
fn old_usage(input: u64, read: u64, write: u64, output: u64) -> Value {
    json!({"input_tokens": input, "cache_read_input_tokens": read, "cache_creation_input_tokens": write, "output_tokens": output})
}

fn assistant(id: &str, model: &str, minute: i64, usage: Value, content: Value, cwd: &Path) -> Value {
    json!({
        "parentUuid": null,
        "isSidechain": false,
        "message": {"id": id, "model": model, "role": "assistant", "type": "message", "content": content, "usage": usage},
        "requestId": format!("req_{id}"),
        "type": "assistant",
        "uuid": format!("u-{id}-{minute}"),
        "timestamp": ts(minute),
        "cwd": cwd.display().to_string(),
    })
}

fn tool_use(id: &str, name: &str, input: Value) -> Value {
    json!([{"type": "tool_use", "id": id, "name": name, "input": input}])
}

fn result(id: &str, minute: i64, content: Value) -> Value {
    json!({"parentUuid": null, "isSidechain": false, "type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": content}]}, "timestamp": ts(minute)})
}

fn jsonl(path: &Path, lines: &[Value]) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    write(path, &text);
}

/// A file of 7,000 bytes, as a read gives it back.
fn file_text() -> String {
    "fn x() {}\n".repeat(700)
}

/// The first session: a read repeated word for word, a cold restart after 70 minutes, a model
/// switch the harness explains with a fallback, an early manual compaction, an inline
/// sub-agent request and a sub-agent transcript of its own, and lines of kinds the report does
/// not know.
fn first_session(dir: &Path, root: &Path) {
    let opus = "claude-opus-5";
    let sonnet = "claude-sonnet-5";
    let lib = root.join("src/lib.rs").display().to_string();
    let lines = vec![
        json!({"type": "permission-mode", "permissionMode": "default"}),
        json!({"type": "user", "isSidechain": false, "message": {"role": "user", "content": "Add a test for the parser."}, "timestamp": ts(0)}),
        // One request streamed as two lines: counted once, with the larger output.
        assistant("m1", opus, 0, usage(10, 0, 30_000, 5, 300), json!([{"type": "thinking", "thinking": "..."}]), root),
        assistant("m1", opus, 0, usage(10, 0, 30_000, 400, 300), tool_use("t1", "Read", json!({"file_path": lib})), root),
        result("t1", 0, json!(file_text())),
        assistant("m2", opus, 1, usage(10, 30_000, 8_000, 200, 100), tool_use("t2", "Read", json!({"file_path": lib})), root),
        result("t2", 1, json!(file_text())),
        assistant("m3", opus, 2, usage(10, 38_000, 2_000, 100, 0), tool_use("t3", "Bash", json!({"command": "cd /tmp/x && cargo test -q"})), root),
        result("t3", 2, json!([{"type": "text", "text": "ok\n".repeat(1166)}, {"type": "image", "source": {"type": "base64", "data": "AAAA"}}])),
        // A sub-agent's request written into the main transcript, and its tool result.
        {
            let mut v = assistant("sc1", opus, 3, usage(5, 0, 1_000, 50, 10), tool_use("t9", "Read", json!({"file_path": lib})), root);
            v["isSidechain"] = json!(true);
            v
        },
        {
            let mut v = result("t9", 3, json!(file_text()));
            v["isSidechain"] = json!(true);
            v
        },
        json!({"type": "mystery-line", "whatever": [1, 2, 3]}),
        // Seventy minutes later the cache has expired.
        assistant("m4", opus, 72, usage(10, 0, 40_000, 100, 50), json!([{"type": "text", "text": "Back."}]), root),
        json!({"type": "system", "subtype": "model_fallback", "originalModel": opus, "fallbackModel": sonnet, "isSidechain": false, "timestamp": ts(73)}),
        assistant("m5", sonnet, 73, usage(10, 0, 41_000, 100, 50), json!([{"type": "text", "text": "On another model."}]), root),
        // An error the harness made up is not a request.
        assistant("e1", "<synthetic>", 73, usage(0, 0, 0, 0, 0), json!([{"type": "text", "text": "API error"}]), root),
        json!({"type": "system", "subtype": "compact_boundary", "isSidechain": false, "timestamp": ts(73), "compactMetadata": {"trigger": "manual", "preTokens": 41_000, "postTokens": 6_000, "durationMs": 9000}}),
        json!({"type": "user", "isSidechain": false, "isCompactSummary": true, "message": {"role": "user", "content": "Summary of the conversation."}, "timestamp": ts(73)}),
        // After a compaction a cold cache is what is expected.
        assistant("m6", sonnet, 74, usage(10, 0, 6_000, 100, 0), json!([{"type": "text", "text": "Fresh."}]), root),
        assistant("m7", sonnet, 75, usage(10, 6_000, 100, 100, 0), json!([{"type": "text", "text": "Done."}]), root),
    ];
    let mut text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    text.push_str("this line is not JSON\n");
    write(&dir.join(format!("{FIRST}.jsonl")), &text);
    let haiku = "claude-haiku-5";
    let sub = |id: &str, minute: i64, u: Value| {
        let mut v = assistant(id, haiku, minute, u, json!([{"type": "text", "text": "found it"}]), root);
        v["isSidechain"] = json!(true);
        v
    };
    jsonl(&dir.join(FIRST).join("subagents/agent-a1.jsonl"), &[sub("a1", 3, usage(5, 0, 2_000, 80, 20)), sub("a2", 4, usage(5, 2_000, 100, 40, 0))]);
}

/// The second session, written by an older version: no thinking tokens, no cache split.
fn second_session(dir: &Path, root: &Path) {
    let opus = "claude-opus-5";
    jsonl(
        &dir.join(format!("{SECOND}.jsonl")),
        &[assistant("r1", opus, 600, old_usage(5, 0, 25_000, 50), json!([]), root), assistant("r2", opus, 603, old_usage(5, 25_000, 500, 60), json!([]), root)],
    );
}

fn run(sb: &Sandbox, root: &Path, args: &[&str]) -> String {
    let mut all = vec!["doctor", "sessions"];
    all.extend(args);
    sb.run_in(root, &all, &[]).ok().to_string()
}

/// The whole JSON report.
fn whole(sb: &Sandbox, root: &Path, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    let out = run(sb, root, &all);
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"))
}

/// The JSON report of one harness.
fn of(sb: &Sandbox, root: &Path, args: &[&str], harness: &str) -> Value {
    let r = whole(sb, root, args);
    r["harnesses"].as_array().and_then(|h| h.iter().find(|x| x["harness"] == harness)).cloned().unwrap_or_else(|| panic!("no {harness} in {r:#}"))
}

/// The JSON report of Claude Code, or the whole one for `--all`.
fn report(sb: &Sandbox, root: &Path, args: &[&str]) -> Value {
    if args.contains(&"--all") { whole(sb, root, args) } else { of(sb, root, args, "Claude Code") }
}

fn near(v: &Value, want: f64) {
    let got = v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"));
    assert!((got - want).abs() <= 1.0, "{got} is not {want}");
}

fn both(tag: &str) -> (Sandbox, PathBuf) {
    let sb = Sandbox::new(tag);
    let root = project(&sb, "demo");
    let dir = transcripts(&sb, &root);
    first_session(&dir, &root);
    second_session(&dir, &root);
    (sb, root)
}

#[test]
fn the_numbers_of_two_sessions() {
    let (sb, root) = both("sessions-numbers");
    let before = sb.snapshot();
    let r = report(&sb, &root, &[]);
    assert_eq!(sb.snapshot(), before, "the report writes nothing");
    // Seven requests in the first session (one of them streamed twice, one made up by the
    // harness and left out) and two in the second.
    assert_eq!((r["sessions"].as_u64(), r["requests"].as_u64()), (Some(2), Some(9)), "{r:#}");
    let w = &r["weighted"];
    near(&w["input"], 80.0);
    near(&w["cache_read"], 0.1 * (74_000.0 + 25_000.0));
    near(&w["cache_write"], 1.25 * (127_100.0 + 25_500.0));
    near(&w["output"], 5.0 * (1_100.0 + 110.0));
    // Sub-agents apart: the one written inline and the two of their own transcript.
    assert_eq!(r["subagents"]["requests"], 3, "{r:#}");
    near(&r["subagents"]["weighted"]["total"], (5.0 + 1_250.0 + 250.0) + (5.0 + 2_500.0 + 400.0) + (5.0 + 200.0 + 125.0 + 200.0));

    // A cold restart after 70 minutes and one after the switch to another model. The
    // request right after the compaction is not one.
    let cold = &r["cold_restarts"];
    assert_eq!(cold["count"], 2, "{cold:#}");
    let cause = |key: &str| cold["by_cause"].as_array().unwrap().iter().find(|c| c["cause"] == key).cloned().unwrap();
    assert_eq!(cause("idle_hour")["count"], 1);
    near(&cause("idle_hour")["weighted"], 1.15 * 40_000.0);
    assert_eq!((cause("model_switch")["count"].as_u64(), cause("model_switch")["after_fallback"].as_u64()), (Some(1), Some(1)));
    near(&cause("model_switch")["weighted"], 1.15 * 40_010.0);
    assert_eq!((cause("idle_five_minutes")["count"].as_u64(), cause("other")["count"].as_u64()), (Some(0), Some(0)));
    assert_eq!(r["model_switches"], 1);

    // The early manual compaction, and its estimate.
    let manual = &r["compactions"]["manual"];
    assert_eq!((manual["count"].as_u64(), manual["early"].as_u64()), (Some(1), Some(1)), "{manual:#}");
    assert_eq!(manual["pre_tokens"], json!([41_000]));
    near(&manual["weighted_estimate"], 0.1 * 41_000.0 + 1.25 * 6_000.0);
    assert_eq!(r["compactions"]["auto"]["count"], 0);

    // Three stretches: the first session's two either side of the compaction, the second's one.
    assert_eq!(r["start"]["first_context_tokens"], json!([30_010, 6_010, 25_005]));
    near(&r["start"]["weighted"], 0.1 * (30_010.0 * 4.0 + 6_010.0 + 25_005.0));

    // Thinking where it is recorded: not in the second session.
    let t = &r["thinking"];
    assert_eq!((t["requests"].as_u64(), t["recorded_requests"].as_u64()), (Some(12), Some(10)), "{t:#}");
    assert_eq!((t["thinking_tokens"].as_u64(), t["output_tokens"].as_u64()), (Some(530), Some(1_270)));
    near(&t["weighted"], 5.0 * 530.0);

    // The second read of the same file in the same stretch, carried by three requests.
    let rep = &r["repeated_reads"];
    assert_eq!((rep["count"].as_u64(), rep["bytes"].as_u64()), (Some(1), Some(7_000)), "{rep:#}");
    near(&rep["weighted"], 7_000.0 / 3.5 * (1.25 + 0.2));

    // Tool results by tool: Bash by its program, images left out, sub-agents' results not here.
    let groups = r["tool_results"]["groups"].as_array().unwrap();
    let group = |name: &str| groups.iter().find(|g| g["tool"] == name).cloned().unwrap_or_else(|| panic!("{name} in {groups:?}"));
    assert_eq!((group("Read")["results"].as_u64(), group("Read")["bytes"].as_u64()), (Some(2), Some(14_000)));
    near(&group("Read")["weighted"], 2_000.0 * 1.55 + 2_000.0 * 1.45);
    assert_eq!(group("Bash cargo")["bytes"], 3_498);
    assert_eq!(groups.len(), 2);
    assert_eq!(r["tool_results"]["largest"][0]["path"], "src/lib.rs");

    let models: Vec<&str> = r["models"].as_array().unwrap().iter().map(|m| m["model"].as_str().unwrap()).collect();
    assert_eq!(models, ["claude-haiku-5", "claude-opus-5", "claude-sonnet-5"]);
}

#[test]
fn the_report_reads_as_findings_by_cost() {
    let (sb, root) = both("sessions-text");
    let text = run(&sb, &root, &[]);
    assert!(text.starts_with("Claude Code sessions of this project in the last 14 days: 2 sessions, 9 main-thread requests, "), "{text}");
    assert!(text.lines().count() < 40, "{text}");
    assert!(text.contains("  sub-agents  ") && text.contains(" 3 requests ("), "{text}");
    assert!(text.contains(". 1 switch in the main thread\n"), "{text}");
    assert!(text.contains("Cold restarts: 2 requests sent the conversation again at the price of new input: 1 model switch (46k, 1 after a model fallback), 1 idle for more than an hour (46k)."), "{text}");
    assert!(text.contains("Come back within the cache lifetime or start fresh. Finish a job on one model.\n"), "{text}");
    assert!(
        text.contains("Manual compactions: 1, at 41k tokens, 1 of them early (under 100k). Each read the conversation and wrote a summary (an estimate).\n"),
        "{text}"
    );
    assert!(text.contains("`/clear` when a job is done instead of a compaction.\n"), "{text}");
    assert!(text.contains("Thinking: 530 of 1k output tokens (42%) (recorded on 10 of 12 requests)"), "{text}");
    assert!(text.contains("Repeated reads: 1 read gave back a file word for word"), "{text}");
    assert!(text.contains("`codefast doctor context` shows what the start holds"), "{text}");
    assert!(text.contains("  largest: Read src/lib.rs (7 KB, "), "{text}");
    // Findings come largest first.
    let costs: Vec<f64> = text
        .split_once("in all:\n")
        .unwrap()
        .1
        .lines()
        .take_while(|l| !l.is_empty())
        .filter(|l| !l.starts_with("               "))
        .map(|l| {
            let n = l.split_whitespace().next().unwrap();
            let (num, mul) = if let Some(k) = n.strip_suffix('k') {
                (k, 1e3)
            } else if let Some(m) = n.strip_suffix('M') {
                (m, 1e6)
            } else {
                (n, 1.0)
            };
            num.parse::<f64>().unwrap() * mul
        })
        .collect();
    assert!(costs.len() >= 4 && costs.windows(2).all(|w| w[0] >= w[1]), "{costs:?}\n{text}");
    assert!(!text.contains('\u{2014}') && !text.contains(';'), "{text}");
}

#[test]
fn one_session_is_broken_down() {
    let (sb, root) = both("sessions-one");
    let text = run(&sb, &root, &["--session", "5f0c"]);
    assert!(
        text.starts_with(&format!("Claude Code session {FIRST}: 7 main-thread requests from 2026-11-10 09:00:00 UTC to 2026-11-10 10:15:00 UTC")),
        "{text}"
    );
    assert!(text.contains("2026-11-10 10:12:00 UTC  idle for more than an hour after 1.2 hours, 40k tokens sent again: 46k"), "{text}");
    assert!(text.contains("2026-11-10 10:13:00 UTC  model switch (a model fallback) after 1 min"), "{text}");
    assert!(text.contains("2026-11-10 10:13:00 UTC  41k tokens down to 6k, early: 12k"), "{text}");
    assert!(text.contains("src/lib.rs  1 more time, 7 KB"), "{text}");
    let r = report(&sb, &root, &["--session", "5f0c"]);
    assert_eq!((r["sessions"].as_u64(), r["cold_restarts"]["items"].as_array().map(Vec::len)), (Some(1), Some(2)), "{r:#}");
    // A prefix that fits no session, or two.
    write(&transcripts(&sb, &root).join("5f0c9999-0000-4000-8000-000000000003.jsonl"), "");
    let out = sb.run_in(&root, &["doctor", "sessions", "--session", "5f0c"], &[]);
    assert!(out.code != 0 && out.stderr.contains("2 sessions start with `5f0c`"), "{}", out.stderr);
    let out = sb.run_in(&root, &["doctor", "sessions", "--session", "zzz"], &[]);
    assert!(out.code != 0 && out.stderr.contains("no Claude Code or Codex session of this project starts with `zzz`"), "{}", out.stderr);
}

#[test]
fn what_an_older_version_does_not_record_is_said_so() {
    let sb = Sandbox::new("sessions-old");
    let root = project(&sb, "old");
    second_session(&transcripts(&sb, &root), &root);
    let text = run(&sb, &root, &[]);
    assert!(text.contains("Not recorded in these transcripts: thinking tokens.\n"), "{text}");
    assert!(!text.contains("Thinking:"), "{text}");
    let r = report(&sb, &root, &[]);
    assert_eq!(r["thinking"]["thinking_tokens"], Value::Null);
    assert_eq!(r["thinking"]["share_of_output"], Value::Null);
    // Without a split the cache writes are priced as five-minute ones.
    near(&r["weighted"]["cache_write"], 1.25 * 25_500.0);
}

#[test]
fn a_project_without_sessions() {
    let sb = Sandbox::new("sessions-none");
    let root = project(&sb, "empty");
    let text = run(&sb, &root, &[]);
    assert_eq!(
        text,
        "No Claude Code or Codex session of this project in the last 14 days. Transcripts are read from ~/.claude/projects/".to_string()
            + &transcripts(&sb, &root).file_name().unwrap().to_string_lossy()
            + " and ~/.codex/sessions.\n"
    );
    let r = report(&sb, &root, &[]);
    assert_eq!((r["sessions"].as_u64(), r["requests"].as_u64(), r["total_weighted"].as_u64()), (Some(0), Some(0), Some(0)));
    assert_eq!(r["findings"], json!([]));
}

#[test]
fn days_and_other_projects() {
    let (sb, root) = both("sessions-days");
    // The second session's transcript was last written a month ago.
    let old = std::fs::OpenOptions::new().write(true).open(transcripts(&sb, &root).join(format!("{SECOND}.jsonl"))).unwrap();
    old.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400)).unwrap();
    drop(old);
    assert_eq!(report(&sb, &root, &[])["sessions"], 1);
    assert_eq!(report(&sb, &root, &["--days", "40"])["sessions"], 2);
    assert_eq!(report(&sb, &root, &["--days=40"])["sessions"], 2);
    let out = sb.run_in(&root, &["doctor", "sessions", "--days", "none"], &[]);
    assert!(out.code != 0 && out.stderr.contains("--days takes a whole number"), "{}", out.stderr);

    // Another project, and every project together.
    let other = project(&sb, "other");
    second_session(&transcripts(&sb, &other), &other);
    let text = run(&sb, &root, &["--all", "--days", "40"]);
    assert!(text.starts_with("Sessions in the last 40 days, by project, largest first:\n"), "{text}");
    assert!(text.contains("  Claude Code  ~/code/demo: 2 sessions, 9 main-thread requests. Largest: "), "{text}");
    assert!(text.contains("  Claude Code  ~/code/other: 1 sessions, 2 main-thread requests"), "{text}");
    assert!(text.contains("  Claude Code  all 2 projects: 3 sessions, 11 main-thread requests"), "{text}");
    let r = report(&sb, &root, &["--all", "--days", "40"]);
    assert_eq!((r["projects"].as_array().map(Vec::len), r["totals"][0]["sessions"].as_u64()), (Some(2), Some(3)), "{r:#}");
    // Another configuration directory is followed.
    let elsewhere = sb.home("elsewhere");
    let out = sb.run_in(&root, &["doctor", "sessions"], &[("CLAUDE_CONFIG_DIR", elsewhere.to_str().unwrap())]);
    assert!(
        out.ok().starts_with("No Claude Code or Codex session of this project in the last 14 days. Transcripts are read from ~/elsewhere/projects/"),
        "{}",
        out.stdout
    );
}

#[test]
fn a_large_conversation_carried_for_long() {
    let sb = Sandbox::new("sessions-carry");
    let root = project(&sb, "long");
    // Ten requests, each reading the one before from the cache, from 150k to 600k tokens.
    let mut lines = Vec::new();
    let mut before = 0u64;
    for i in 0..10u64 {
        let context = 150_000 + 50_000 * i;
        lines.push(assistant(&format!("c{i}"), "claude-opus-5", i as i64, usage(0, before, context - before, 10, 0), json!([]), &root));
        before = context;
    }
    jsonl(&transcripts(&sb, &root).join(format!("{FIRST}.jsonl")), &lines);
    let r = report(&sb, &root, &[]);
    let c = &r["carried_context"];
    assert_eq!(
        (c["threshold"].as_u64(), c["requests_above"].as_u64(), c["requests"].as_u64(), c["peak_context"].as_u64()),
        (Some(200_000), Some(8), Some(10), Some(600_000)),
        "{c:#}"
    );
    // 50k + 100k + ... + 400k above the threshold, read again at 0.1.
    near(&c["weighted"], 0.1 * 1_800_000.0);
    // Compacting at 200k down to the 25k of a session without compactions: the context would
    // repeat every 175k, so 250k to 600k become 75k, 125k, 175k, 50k, 100k, 150k, 25k, 75k,
    // with a compaction at 250k, 400k and 550k. The reads saved are a tenth of what is left out.
    let saved = 0.1 * (175_000.0 * 3.0 + 350_000.0 * 3.0 + 525_000.0 * 2.0);
    assert_eq!(c["compactions_instead"], 3);
    near(&c["saved_reads_estimate"], saved);
    near(&c["compactions_weighted_estimate"], 3.0 * (0.1 * 200_000.0 + 1.25 * 25_000.0));
    near(&c["difference_estimate"], saved - 153_750.0);
    assert!(r["findings"].as_array().unwrap().iter().any(|f| f["finding"] == "carried_context"), "{r:#}");
    let text = run(&sb, &root, &[]);
    assert!(
        text.contains("Carried context: 8 of 10 requests (80%) carried more than 200k tokens, at most 600k. Reading the part above 200k again cost 180k.\n"),
        "{text}"
    );
    assert!(text.contains("Compacting at 200k instead: 3 compactions, about 154k, so about 109k less (an estimate).\n"), "{text}");
    assert!(
        text.contains("Start a fresh session at a natural break, or compact earlier, when the context stays far above what the next step needs.\n"),
        "{text}"
    );
    assert!(text.contains("Output and thinking are the counts the transcript recorded."), "{text}");
    // A model the price table does not name is weighed by the defaults, and the report says so.
    assert!(text.contains("claude-opus-5: a cache read 0.1, a cache write 1.25 (2 for one hour), output 5. "), "{text}");
    // Above the peak there is nothing to say.
    let r = report(&sb, &root, &["--carry", "700000"]);
    assert_eq!((r["carried_context"]["requests_above"].as_u64(), r["carried_context"]["weighted"].as_u64()), (Some(0), Some(0)));
    assert!(!r["findings"].as_array().unwrap().iter().any(|f| f["finding"] == "carried_context"), "{r:#}");
    assert!(!run(&sb, &root, &["--carry", "700000"]).contains("Carried context"));
    // The small sessions of the other tests stay below it as well.
    let (sb, root) = both("sessions-carry-small");
    assert!(!report(&sb, &root, &[])["findings"].as_array().unwrap().iter().any(|f| f["finding"] == "carried_context"));
}

#[test]
fn the_harness_reason_for_a_cache_miss() {
    let sb = Sandbox::new("sessions-miss");
    let root = project(&sb, "miss");
    let opus = "claude-opus-5";
    let with = |mut v: Value, reason: &str| {
        v["message"]["diagnostics"] = json!({"cache_miss_reason": {"type": reason, "cache_missed_input_tokens": 1000}});
        v
    };
    let lines = [
        assistant("q0", opus, 0, usage(10, 0, 40_000, 10, 0), json!([]), &root),
        // Seventy minutes away, and the harness says the messages changed.
        with(assistant("q1", opus, 70, usage(10, 0, 40_100, 10, 0), json!([]), &root), "messages_changed"),
        // A warm request with a reason: never printed.
        with(assistant("q2", opus, 71, usage(10, 40_100, 100, 10, 0), json!([]), &root), "tools_changed"),
        // A switch of model, and the harness agrees.
        with(assistant("q3", "claude-sonnet-5", 72, usage(10, 0, 40_300, 10, 0), json!([]), &root), "model_changed"),
    ];
    jsonl(&transcripts(&sb, &root).join(format!("{FIRST}.jsonl")), &lines);
    let text = run(&sb, &root, &["--session", "5f0c"]);
    assert!(text.contains("idle for more than an hour (harness: messages_changed) after 1.2 hours"), "{text}");
    assert!(text.contains("model switch (harness: model_changed) after 1 min"), "{text}");
    assert!(!text.contains("tools_changed"), "{text}");
    // One of two names another cause: more than a quarter.
    let text = run(&sb, &root, &[]);
    assert!(text.contains("1 model switch (46k), 1 idle for more than an hour (46k), and the harness names another cause for 1 of them.\n"), "{text}");
    let r = report(&sb, &root, &["--session", "5f0c"]);
    assert_eq!(r["cold_restarts"]["harness_names_another_cause"], 1);
    let reasons: Vec<&Value> = r["cold_restarts"]["items"].as_array().unwrap().iter().map(|c| &c["harness_reason"]).collect();
    assert_eq!(reasons, [&json!("messages_changed"), &json!("model_changed")]);
    // Where the two agree, nothing is added.
    let (sb, root) = both("sessions-miss-none");
    assert!(!run(&sb, &root, &[]).contains("the harness names"));
}

// ----- Codex -----

const ROLLOUT: &str = "01a1d94c-0000-7000-8000-00000000000a";
const OUTSIDE: &str = "01a1d94c-0000-7000-8000-00000000000b";

/// A rollout line.
fn cx(minute: i64, kind: &str, payload: Value) -> Value {
    json!({"timestamp": ts(minute), "ordinal": minute, "type": kind, "payload": payload})
}

fn meta(cwd: &Path) -> Value {
    cx(
        0,
        "session_meta",
        json!({"id": "x", "cwd": cwd.display().to_string(), "cli_version": "0.164.0", "model_provider": "openai", "base_instructions": {"text": "..."}}),
    )
}

fn model(minute: i64, name: &str) -> Value {
    cx(
        minute,
        "turn_context",
        json!({"model": name, "cwd": "/somewhere", "collaboration_mode": {"mode": "default", "settings": {"model": name, "reasoning_effort": "medium"}}}),
    )
}

/// One request: `input` is everything sent, `cached` and `written` parts of it. `total` is the
/// running total the harness keeps.
fn tokens(minute: i64, input: u64, cached: u64, written: u64, output: u64, reasoning: u64, total: u64) -> Value {
    let last = json!({"input_tokens": input, "cached_input_tokens": cached, "cache_write_input_tokens": written, "output_tokens": output, "reasoning_output_tokens": reasoning, "total_tokens": input + output});
    cx(
        minute,
        "event_msg",
        json!({"type": "token_count", "info": {"last_token_usage": last, "total_token_usage": {"total_tokens": total}, "model_context_window": 258_400}, "rate_limits": null}),
    )
}

fn shell(minute: i64, id: &str, script: &str) -> Value {
    let args = json!({"command": ["bash", "-lc", script]}).to_string();
    cx(minute, "response_item", json!({"type": "function_call", "name": "shell", "arguments": args, "call_id": id}))
}

fn output(minute: i64, id: &str, text: &str) -> Value {
    cx(minute, "response_item", json!({"type": "function_call_output", "call_id": id, "output": text}))
}

fn rollout_path(sb: &Sandbox, id: &str) -> PathBuf {
    sb.home(&format!(".codex/sessions/2026/11/10/rollout-2026-11-10T09-00-00-{id}.jsonl"))
}

/// A Codex session run in a directory inside the project: a report of nothing, a request
/// reported twice, a repeated shell output, a cold restart after eight minutes, a model switch,
/// a compaction said twice and a line of a kind the report does not know. And a session run
/// in another directory.
fn codex_sessions(sb: &Sandbox, root: &Path) {
    let inside = root.join("sub");
    std::fs::create_dir_all(&inside).unwrap();
    let log = "ok\n".repeat(4000);
    jsonl(
        &rollout_path(sb, ROLLOUT),
        &[
            meta(&inside),
            model(0, "gpt-6"),
            tokens(0, 0, 0, 0, 0, 0, 0),
            shell(0, "c1", "cd x && cargo test"),
            output(0, "c1", &log),
            tokens(0, 30_000, 0, 30_000, 500, 300, 30_500),
            tokens(0, 30_000, 0, 30_000, 500, 300, 30_500),
            shell(1, "c2", "cd x && cargo test"),
            output(1, "c2", &log),
            tokens(1, 31_000, 30_000, 1_000, 200, 100, 61_700),
            // Eight minutes away, and this provider reports no cache writes.
            tokens(9, 32_000, 0, 0, 100, 0, 93_800),
            model(10, "gpt-6-mini"),
            tokens(10, 33_000, 1_000, 30_000, 100, 0, 126_900),
            cx(10, "compacted", json!({"message": "summary"})),
            cx(10, "event_msg", json!({"type": "context_compacted"})),
            tokens(11, 8_000, 0, 8_000, 100, 0, 135_000),
            cx(11, "mystery_line", json!({"whatever": 1})),
        ],
    );
    let elsewhere = sb.home("code/elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    jsonl(&rollout_path(sb, OUTSIDE), &[meta(&elsewhere), model(0, "gpt-6"), tokens(0, 50_000, 0, 50_000, 100, 0, 50_100)]);
}

#[test]
fn a_codex_session_in_numbers() {
    let sb = Sandbox::new("sessions-codex");
    let root = project(&sb, "cx");
    codex_sessions(&sb, &root);
    let before = sb.snapshot();
    let r = of(&sb, &root, &[], "Codex");
    assert_eq!(sb.snapshot(), before, "the report writes nothing");
    // The session in the other directory is not this project's. One request per report of
    // tokens: not the empty one, and the one reported twice only once.
    assert_eq!((r["sessions"].as_u64(), r["requests"].as_u64()), (Some(1), Some(5)), "{r:#}");
    let w = &r["weighted"];
    near(&w["cache_read"], 0.1 * 31_000.0);
    near(&w["cache_write"], 1.25 * 69_000.0);
    near(&w["input"], 32_000.0 + 2_000.0);
    near(&w["output"], 5.0 * 1_000.0);
    // Eight minutes away is a cold restart with no one-hour cache to tell apart, paid as
    // uncached input. Then a switch of model. Nothing after the compaction.
    let cold = &r["cold_restarts"];
    assert_eq!(cold["count"], 2, "{cold:#}");
    let cause = |key: &str| cold["by_cause"].as_array().unwrap().iter().find(|c| c["cause"] == key).cloned().unwrap();
    assert_eq!((cause("idle_five_minutes")["count"].as_u64(), cause("idle_hour")["count"].as_u64()), (Some(1), Some(0)));
    near(&cause("idle_five_minutes")["weighted"], 0.9 * 31_000.0);
    near(&cause("model_switch")["weighted"], 1.15 * 30_000.0 + 0.9 * 2_000.0);
    assert_eq!(r["model_switches"], 1);
    // The compaction, said twice, is one boundary, sized by the requests either side of it.
    let c = &r["compactions"]["auto"];
    assert_eq!((c["count"].as_u64(), c["pre_tokens"].clone()), (Some(1), json!([33_000])), "{c:#}");
    near(&c["weighted_estimate"], 0.1 * 33_000.0 + 1.25 * 8_000.0);
    assert_eq!(r["start"]["first_context_tokens"], json!([30_000, 8_000]));
    // Thinking is the reasoning tokens, as a part of the output.
    let t = &r["thinking"];
    assert_eq!((t["recorded_requests"].as_u64(), t["thinking_tokens"].as_u64(), t["output_tokens"].as_u64()), (Some(5), Some(400), Some(1_000)), "{t:#}");
    // The shell's results by the program it ran, and the second, same output of the same call.
    let groups = r["tool_results"]["groups"].as_array().unwrap();
    assert_eq!((groups.len(), groups[0]["tool"].as_str(), groups[0]["results"].as_u64()), (1, Some("shell cargo"), Some(2)), "{groups:?}");
    let rep = &r["repeated_outputs"];
    assert_eq!((rep["count"].as_u64(), rep["bytes"].as_u64()), (Some(1), Some(12_000)), "{r:#}");
    // Two requests carry it before the compaction.
    near(&rep["weighted"], 12_000.0 / 3.5 * (1.25 + 0.1 * 2.0));
    assert!(r["transcripts"].as_str().unwrap().ends_with("/.codex/sessions"));

    let text = run(&sb, &root, &[]);
    assert!(text.starts_with("Codex sessions of this project in the last 14 days: 1 session, 5 main-thread requests, "), "{text}");
    assert!(text.contains("(Weighted by the defaults, since Codex providers price differently"), "{text}");
    assert!(text.contains("1 idle for more than five minutes ("), "{text}");
    assert!(
        text.contains("Repeated outputs: 1 call gave back word for word what the same call with the same arguments gave earlier in the stretch (12 KB)."),
        "{text}"
    );
    assert!(text.contains("Thinking: 400 of 1k output tokens (40%), counted as output, at 5 times the weight of input."), "{text}");
    assert!(!text.contains("Claude Code sessions"), "{text}");
    // A Codex session by the start of its id, and `CODEX_HOME` followed.
    let one = run(&sb, &root, &["--session", "01a1d94c-0000-7000-8000-00000000000a"]);
    assert!(one.starts_with(&format!("Codex session {ROLLOUT}: 5 main-thread requests from ")), "{one}");
    let out = sb.run_in(&root, &["doctor", "sessions"], &[("CODEX_HOME", sb.home("nowhere").to_str().unwrap())]);
    assert!(out.ok().starts_with("No Claude Code or Codex session of this project"), "{}", out.stdout);
}

#[test]
fn codex_carried_context() {
    let sb = Sandbox::new("sessions-codex-carry");
    let root = project(&sb, "cxlong");
    let mut lines = vec![meta(&root), model(0, "gpt-6")];
    let (mut before, mut total) = (0u64, 0u64);
    for i in 0..10u64 {
        let context = 150_000 + 50_000 * i;
        total += context + 10;
        lines.push(tokens(i as i64, context, before, context - before, 10, 0, total));
        before = context;
    }
    jsonl(&rollout_path(&sb, ROLLOUT), &lines);
    let c = of(&sb, &root, &[], "Codex")["carried_context"].clone();
    assert_eq!((c["requests_above"].as_u64(), c["peak_context"].as_u64(), c["compactions_instead"].as_u64()), (Some(8), Some(600_000), Some(3)), "{c:#}");
    near(&c["weighted"], 180_000.0);
    near(&c["saved_reads_estimate"], 262_500.0);
    assert!(run(&sb, &root, &[]).contains("Compacting at 200k instead: 3 compactions, about 154k, so about 109k less (an estimate).\n"));
}

#[test]
fn a_project_with_sessions_of_both_harnesses() {
    let (sb, root) = both("sessions-both");
    codex_sessions(&sb, &root);
    let text = run(&sb, &root, &[]);
    let claude = text.find("Claude Code sessions of this project").expect("a Claude Code report");
    let codex = text.find("\nCodex sessions of this project").expect("a Codex report");
    assert!(claude == 0 && codex > claude, "{text}");
    let r = whole(&sb, &root, &[]);
    let names: Vec<&str> = r["harnesses"].as_array().unwrap().iter().map(|h| h["harness"].as_str().unwrap()).collect();
    assert_eq!(names, ["Claude Code", "Codex"]);
    // Every project of either harness, each line naming its harness.
    let all = run(&sb, &root, &["--all"]);
    assert!(all.contains("  Claude Code  ~/code/demo: 2 sessions, 9 main-thread requests"), "{all}");
    assert!(all.contains("  Codex        ~/code/demo/sub: 1 sessions, 5 main-thread requests"), "{all}");
    assert!(all.contains("  Codex        ~/code/elsewhere: 1 sessions, 1 main-thread requests"), "{all}");
    assert!(all.contains("  Codex        all 2 projects: 2 sessions, 6 main-thread requests"), "{all}");
    let r = whole(&sb, &root, &["--all"]);
    let totals: Vec<&str> = r["totals"].as_array().unwrap().iter().map(|t| t["harness"].as_str().unwrap()).collect();
    assert_eq!(totals, ["Claude Code", "Codex"]);
}

// ----- prices by model -----

/// A session of `model`: requests whose context grows by the steps given, each reading the one
/// before from the cache and writing what is new.
fn growing(sb: &Sandbox, root: &Path, id: &str, model: &str, contexts: &[u64]) {
    let mut lines = Vec::new();
    let mut before = 0;
    for (i, c) in contexts.iter().enumerate() {
        lines.push(assistant(&format!("{id}-{i}"), model, i as i64, usage(0, before, c - before, 100, 0), json!([]), root));
        before = *c;
    }
    jsonl(&transcripts(sb, root).join(format!("{id}.jsonl")), &lines);
}

#[test]
fn cache_reads_follow_the_model() {
    let sb = Sandbox::new("sessions-prices");
    let root = project(&sb, "prices");
    growing(&sb, &root, "5f0c2a4e-0000-4000-8000-0000000000f1", "claude-fable-5-2", &[30_000, 31_000]);
    growing(&sb, &root, "5f0c2a4e-0000-4000-8000-0000000000f2", "claude-sonnet-5-5", &[30_000, 31_000]);
    let r = report(&sb, &root, &[]);
    // 30,000 tokens read once in each: at 0.025 on Fable 5.2 and at 0.05 on Sonnet 5.5.
    near(&r["weighted"]["cache_read"], 0.025 * 30_000.0 + 0.05 * 30_000.0);
    near(&r["weighted"]["cache_write"], 1.25 * 62_000.0);
    near(&r["start"]["weighted"], 0.025 * 30_000.0 + 0.05 * 30_000.0);
    assert_eq!(r["weights"]["claude-fable-5-2"]["cache_read"], 0.025, "{r:#}");
    assert_eq!(r["weights"]["claude-sonnet-5-5"]["cache_read"], 0.05);
    assert_eq!((r["weights"]["claude-sonnet-5-5"]["cache_write_1h"].as_f64(), r["weights"]["claude-sonnet-5-5"]["tier"].clone()), (Some(2.0), Value::Null));
    let text = run(&sb, &root, &[]);
    assert!(text.contains("(Weights follow each model's published prices of 2026-11-15, in its own uncached input tokens: "), "{text}");
    assert!(text.contains("claude-fable-5-2: a cache read 0.025, a cache write 1.25 (2 for one hour), output 5"), "{text}");
    assert!(text.contains("claude-sonnet-5-5: a cache read 0.05, a cache write 1.25 (2 for one hour), output 5"), "{text}");
}

#[test]
fn haiku_steps_up_over_a_prompt_of_100k() {
    let sb = Sandbox::new("sessions-haiku");
    let root = project(&sb, "haiku");
    // 60k, 80k, exactly 100k (still the lower price), then 120k and 140k at five times.
    growing(&sb, &root, FIRST, "claude-haiku-5-5", &[60_000, 80_000, 100_000, 120_000, 140_000]);
    let r = report(&sb, &root, &[]);
    let at = |read: f64, write: f64| 0.1 * read + 1.25 * write + 5.0 * 100.0;
    let lower = [at(0.0, 60_000.0), at(60_000.0, 20_000.0), at(80_000.0, 20_000.0), at(100_000.0, 20_000.0), at(120_000.0, 20_000.0)];
    near(&r["total_weighted"], lower[..3].iter().sum::<f64>() + 5.0 * (lower[3] + lower[4]));
    // The step: the two requests over 100k, what they paid and what they would have paid.
    let step = &r["price_step"];
    assert_eq!((step["requests"].as_u64(), step["over_tokens"].as_u64()), (Some(2), Some(100_000)), "{step:#}");
    near(&step["weighted"], 5.0 * (lower[3] + lower[4]));
    near(&step["weighted_at_lower_price"], lower[3] + lower[4]);
    near(&step["step_weighted"], 4.0 * (lower[3] + lower[4]));
    assert_eq!(r["findings"][0]["finding"], "price_step", "{r:#}");
    // Carried context is measured from the step, at the price each request paid.
    let c = &r["carried_context"];
    assert_eq!((c["thresholds"].clone(), c["requests_above"].as_u64()), (json!([100_000]), Some(2)), "{c:#}");
    near(&c["weighted"], 5.0 * 0.1 * (20_000.0 + 40_000.0));
    // Compacted at 100k down to 25k, the two would have carried 45k and 65k, below the step:
    // one compaction at the lower price, and the reads saved at the price each length pays.
    assert_eq!(c["compactions_instead"], 1);
    near(&c["compactions_weighted_estimate"], 0.1 * 100_000.0 + 1.25 * 25_000.0);
    near(&c["saved_reads_estimate"], 0.1 * (5.0 * 120_000.0 - 45_000.0) + 0.1 * (5.0 * 140_000.0 - 65_000.0));
    assert_eq!(r["weights"]["claude-haiku-5-5"]["tier"], json!({"over_tokens": 100_000, "times": 5.0}));

    let text = run(&sb, &root, &[]);
    assert!(
        text.contains(
            "Price step: 2 requests of claude-haiku-5-5 had a prompt over 100k tokens and cost 365k, where the lower price gives 73k: the step cost 292k.\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("Keep a conversation with this model under 100k tokens, by starting a fresh session or compacting before it gets there.\n"),
        "{text}"
    );
    assert!(text.contains("Carried context: 2 of 5 requests (40%) carried more than 100k tokens, at most 140k."), "{text}");
    assert!(text.contains("Compacting at 100k instead: 1 compaction, about 41k, so about 78k less (an estimate).\n"), "{text}");
    assert!(
        text.contains("claude-haiku-5-5: a cache read 0.1, a cache write 1.25 (2 for one hour), output 5, and 5 times each over a prompt of 100k tokens"),
        "{text}"
    );
    // `--carry` given is the threshold whatever the model.
    let given = report(&sb, &root, &["--carry", "200000"]);
    assert_eq!((given["carried_context"]["thresholds"].clone(), given["carried_context"]["requests_above"].as_u64()), (json!([200_000]), Some(0)));
}
