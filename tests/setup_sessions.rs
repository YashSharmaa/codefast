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

fn report(sb: &Sandbox, root: &Path, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    let out = run(sb, root, &all);
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}"))
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
    assert!(out.code != 0 && out.stderr.contains("no Claude Code session of this project starts with `zzz`"), "{}", out.stderr);
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
        "No Claude Code session of this project in the last 14 days. Transcripts are read from ~/.claude/projects/".to_string()
            + &transcripts(&sb, &root).file_name().unwrap().to_string_lossy()
            + ".\n"
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
    assert!(text.starts_with("Claude Code sessions in the last 40 days, by project, largest first:\n"), "{text}");
    assert!(text.contains("  ~/code/demo: 2 sessions, 9 main-thread requests. Largest: "), "{text}");
    assert!(text.contains("  ~/code/other: 1 sessions, 2 main-thread requests"), "{text}");
    assert!(text.contains("  all 2 projects: 3 sessions, 11 main-thread requests"), "{text}");
    let r = report(&sb, &root, &["--all", "--days", "40"]);
    assert_eq!((r["projects"].as_array().map(Vec::len), r["total"]["sessions"].as_u64()), (Some(2), Some(3)), "{r:#}");
    // Another configuration directory is followed.
    let elsewhere = sb.home("elsewhere");
    let out = sb.run_in(&root, &["doctor", "sessions"], &[("CLAUDE_CONFIG_DIR", elsewhere.to_str().unwrap())]);
    assert!(
        out.ok().starts_with("No Claude Code session of this project in the last 14 days. Transcripts are read from ~/elsewhere/projects/"),
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
    assert!(text.contains("Output and thinking are the counts the transcript recorded, the largest of the lines of a request."), "{text}");
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
