//! The stateless era of the MCP server (revision 2026-07-28): every request carries its own
//! `_meta`, there is no handshake, and field names follow the official schema.

#[path = "packaging/support.rs"]
mod support;

use serde_json::{Value, json};
use std::time::Duration;
use support::*;

fn assert_modern_result(reply: &Value) {
    let result = &reply["result"];
    assert_eq!(result["resultType"], "complete", "every result carries resultType: {reply}");
    let info = &result["_meta"][META_SERVER];
    assert_eq!(info["name"], "codefast", "serverInfo travels in the result's _meta: {reply}");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn discover_reports_versions_capabilities_and_identity() {
    let repo = Repo::new("discover");
    let mut s = Session::in_repo(&repo);
    let reply = s.modern("server/discover", Value::Null);
    assert_modern_result(&reply);
    let r = &reply["result"];
    assert_eq!(r["supportedVersions"], json!([MODERN]));
    assert!(r["capabilities"]["tools"].is_object(), "the tools capability is declared: {r}");
    assert!(r["capabilities"].get("resources").is_none() && r["capabilities"].get("prompts").is_none());
    assert!(r["ttlMs"].as_u64().is_some(), "ttlMs is a non-negative integer: {r}");
    assert_eq!(r["cacheScope"], "public");
    let instructions = r["instructions"].as_str().expect("instructions");
    assert!(instructions.starts_with("codefast"), "the purpose comes first: {instructions}");
    assert!(instructions.len() <= 4096, "Claude Code cuts instructions at 4,096 characters");
    assert!(!instructions.contains("`root`"), "inside a repository no root needs to be passed");
    // The legacy identity field does not exist in this revision.
    assert!(r.get("serverInfo").is_none() && r.get("protocolVersion").is_none());
}

#[test]
fn tools_list_is_cacheable_complete_and_compact() {
    let repo = Repo::new("list");
    let mut s = Session::in_repo(&repo);
    let reply = s.modern("tools/list", Value::Null);
    assert_modern_result(&reply);
    let r = &reply["result"];
    assert!(r["ttlMs"].as_u64().is_some_and(|t| t > 0), "list results carry ttlMs: {r}");
    assert_eq!(r["cacheScope"], "public");
    assert!(r.get("nextCursor").is_none(), "a single page has no cursor");

    let tools = r["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["search", "show", "refs", "path", "grep"], "the tool list and its order are part of the product");
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(name.len() <= 128 && name.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)));
        let description = t["description"].as_str().unwrap();
        assert!(!description.is_empty() && description.len() <= 512, "{name}: descriptions stay short");
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(t["inputSchema"]["required"].as_array().is_some_and(|r| !r.is_empty()));
        assert_eq!(t["inputSchema"]["properties"]["root"]["type"], "string", "{name} accepts a root");
        assert_eq!(t["inputSchema"]["properties"]["budget"]["type"], "integer", "{name} accepts a budget");
        let a = &t["annotations"];
        assert_eq!(a["readOnlyHint"], true, "{name}");
        assert_eq!(a["destructiveHint"], false, "{name}");
        assert_eq!(a["idempotentHint"], true, "{name}");
        assert_eq!(a["openWorldHint"], false, "{name}");
        assert!(a["title"].is_string(), "{name}");
        let always = t["_meta"]["anthropic/alwaysLoad"] == json!(true);
        assert_eq!(always, name == "search", "only the entry point loads up front");
        assert!(t.get("outputSchema").is_none(), "answers are plain text");
        // Nothing outside the schema's `Tool` and `ToolAnnotations`.
        for key in t.as_object().unwrap().keys() {
            assert!(["name", "description", "inputSchema", "annotations", "_meta"].contains(&key.as_str()), "{name}: unexpected key {key}");
        }
        for key in a.as_object().unwrap().keys() {
            assert!(
                ["title", "readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"].contains(&key.as_str()),
                "{name}: unexpected annotation {key}"
            );
        }
        for (argument, schema) in t["inputSchema"]["properties"].as_object().unwrap() {
            assert!(schema["type"].is_string(), "{name}.{argument} declares a type");
            assert!(schema["description"].as_str().is_none_or(|d| d.len() <= 100), "{name}.{argument}: parameter descriptions stay short");
        }
        for required in t["inputSchema"]["required"].as_array().unwrap() {
            assert!(t["inputSchema"]["properties"].get(required.as_str().unwrap()).is_some(), "{name}: {required} is required but not declared");
        }
    }
    for key in r.as_object().unwrap().keys() {
        assert!(["tools", "ttlMs", "cacheScope", "resultType", "_meta"].contains(&key.as_str()), "unexpected key {key} in a ListToolsResult");
    }

    // Deterministic: a second request, and the handshake era, return byte-identical definitions.
    let again = s.modern("tools/list", Value::Null);
    assert_eq!(again["result"]["tools"].to_string(), r["tools"].to_string());
    let legacy = s.legacy("tools/list", Value::Null);
    assert_eq!(legacy["result"]["tools"].to_string(), r["tools"].to_string());

    let size = reply.to_string().len();
    assert!(size < 5200, "tools/list grew to {size} bytes; keep the definitions compact");
}

#[test]
fn tools_list_rejects_a_cursor() {
    let repo = Repo::new("cursor");
    let mut s = Session::in_repo(&repo);
    let reply = s.modern("tools/list", json!({"cursor": "page-2"}));
    assert_eq!(error_code(&reply), -32602);
}

#[test]
fn every_tool_answers_in_plain_text() {
    let repo = Repo::new("tools");
    let mut s = Session::in_repo(&repo);

    let reply = s.modern("tools/call", json!({"name": "search", "arguments": {"queries": ["dev_read"]}}));
    assert_modern_result(&reply);
    let r = &reply["result"];
    assert_eq!(r["isError"], false, "{r}");
    assert_eq!(r["content"][0]["type"], "text");
    assert!(text(r).contains("src/io.c:4"), "definition location: {}", text(r));
    assert!(text(r).contains("copy_all"), "callers come with the definition: {}", text(r));
    assert!(r.get("structuredContent").is_none());

    let r = s.call("search", json!({"queries": ["dev_read", "dev_write"], "budget": 400}));
    assert!(text(&r).contains("dev_read") && text(&r).contains("dev_write"), "several queries in one call: {}", text(&r));

    let r = s.call("show", json!({"targets": ["src/main.c"], "outline": true}));
    assert!(text(&r).contains("copy_all") && text(&r).contains("main"), "{}", text(&r));

    let r = s.call("show", json!({"targets": ["src/io.c:4-9"]}));
    assert!(text(&r).contains("return d->ops->read"), "{}", text(&r));

    let r = s.call("refs", json!({"targets": ["dev_write"]}));
    assert!(text(&r).contains("src/main.c"), "{}", text(&r));
    let r = s.call("refs", json!({"targets": ["dev_read"], "mode": "callers", "depth": 2}));
    assert!(text(&r).contains("copy_all"), "{}", text(&r));
    let r = s.call("refs", json!({"targets": ["copy_all"], "mode": "callees"}));
    assert!(text(&r).contains("dev_read") && text(&r).contains("dev_write"), "{}", text(&r));

    let r = s.call("path", json!({"from": "main", "to": "dev_write"}));
    assert!(text(&r).contains("copy_all"), "the chain goes through copy_all: {}", text(&r));

    let r = s.call("grep", json!({"pattern": "dev_write: %d bytes", "fixed": true}));
    assert_eq!(r["isError"], false);
    assert!(text(&r).contains("src/io.c"), "{}", text(&r));

    let r = s.call("search", json!({"queries": ["dev_read"], "path": "lib/"}));
    assert!(!text(&r).contains("src/io.c:4-9"), "the path filter applies: {}", text(&r));
}

#[test]
fn no_handshake_is_needed_and_both_eras_share_one_process() {
    let repo = Repo::new("mixed");
    let mut s = Session::in_repo(&repo);
    // Straight to a tool call: nothing was negotiated.
    let r = s.call("search", json!({"queries": [repo.marker]}));
    assert!(text(&r).contains("lib/marker.c"), "{}", text(&r));
    // A legacy client may open the same process later; modern requests stay modern.
    let init = s.legacy("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}));
    assert!(init["result"].get("resultType").is_none(), "legacy results have no resultType: {init}");
    let reply = s.modern("tools/list", Value::Null);
    assert_modern_result(&reply);
    let legacy = s.legacy("tools/list", Value::Null);
    assert!(legacy["result"].get("resultType").is_none() && legacy["result"].get("ttlMs").is_none(), "{legacy}");
    assert!(legacy["result"].get("_meta").is_none());
}

#[test]
fn unsupported_version_names_the_supported_ones() {
    let repo = Repo::new("version");
    let mut s = Session::in_repo(&repo);
    for requested in ["2099-01-01", "2025-11-25", "DRAFT-2026-v1"] {
        s.send(&json!({"jsonrpc": "2.0", "id": "v", "method": "tools/list", "params": {"_meta": {META_VERSION: requested, META_CAPS: {}}}}));
        let reply = s.recv();
        assert_eq!(error_code(&reply), -32022, "{reply}");
        assert_eq!(reply["id"], "v");
        assert_eq!(reply["error"]["data"]["supported"], json!([MODERN]));
        assert_eq!(reply["error"]["data"]["requested"], requested);
    }
    // The probe of a dual-era client gets the same recognisable error, not a legacy answer.
    s.send(&json!({"jsonrpc": "2.0", "id": 9, "method": "server/discover", "params": {"_meta": {META_VERSION: "2027-01-01", META_CAPS: {}}}}));
    assert_eq!(error_code(&s.recv()), -32022);
}

#[test]
fn required_request_metadata_is_enforced() {
    let repo = Repo::new("meta");
    let mut s = Session::in_repo(&repo);
    // protocolVersion without clientCapabilities: malformed.
    s.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": {META_VERSION: MODERN}}}));
    let reply = s.recv();
    assert_eq!(error_code(&reply), -32602, "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("clientCapabilities"));
    // clientCapabilities of the wrong type.
    s.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": {META_VERSION: MODERN, META_CAPS: []}}}));
    assert_eq!(error_code(&s.recv()), -32602);
    // A version that is not a string.
    s.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {"_meta": {META_VERSION: 20260728, META_CAPS: {}}}}));
    assert_eq!(error_code(&s.recv()), -32602);
    // server/discover exists only in the stateless era, so it cannot come without metadata.
    s.send(&json!({"jsonrpc": "2.0", "id": 4, "method": "server/discover"}));
    let reply = s.recv();
    assert_eq!(error_code(&reply), -32602, "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains(MODERN), "the error names the supported revision");
    // The server is still healthy.
    assert!(s.modern("server/discover", Value::Null)["result"]["supportedVersions"].is_array());
}

#[test]
fn methods_removed_from_the_revision_are_not_found() {
    let repo = Repo::new("removed");
    let mut s = Session::in_repo(&repo);
    for method in [
        "ping",
        "logging/setLevel",
        "resources/subscribe",
        "resources/list",
        "resources/read",
        "prompts/list",
        "prompts/get",
        "completion/complete",
        "roots/list",
        "nothing/at-all",
    ] {
        let reply = s.modern(method, Value::Null);
        assert_eq!(error_code(&reply), -32601, "{method}: {reply}");
    }
    let reply = s.modern("prompts/list", Value::Null);
    assert_eq!(reply["error"]["message"], "Prompts not supported");
}

#[test]
fn unknown_tools_and_malformed_calls_are_protocol_errors() {
    let repo = Repo::new("unknown");
    let mut s = Session::in_repo(&repo);
    let reply = s.modern("tools/call", json!({"name": "delete_everything", "arguments": {}}));
    assert_eq!(error_code(&reply), -32602);
    assert_eq!(reply["error"]["message"], "Unknown tool: delete_everything");
    assert_eq!(error_code(&s.modern("tools/call", json!({"arguments": {}}))), -32602, "name is required");
    assert_eq!(error_code(&s.modern("tools/call", json!({"name": "search", "arguments": ["dev_read"]}))), -32602, "arguments must be an object");
}

#[test]
fn mistakes_the_model_can_fix_are_tool_errors() {
    let repo = Repo::new("fixable");
    let mut s = Session::in_repo(&repo);
    let cases = [
        ("search", json!({}), "`queries` is required"),
        ("search", json!({"queries": []}), "`queries` is required"),
        ("search", json!({"queries": [1, 2]}), "array of strings"),
        ("search", json!({"queries": ["x"], "budget": "lots"}), "`budget`"),
        ("show", json!({"targets": {"a": 1}}), "`targets`"),
        ("refs", json!({"targets": ["dev_read"], "mode": "sideways"}), "unknown mode `sideways`"),
        ("refs", json!({"targets": ["dev_read"], "mode": "callers", "depth": -2}), "`depth`"),
        ("path", json!({"from": "main"}), "`to` is required"),
        ("grep", json!({}), "`pattern` is required"),
        ("grep", json!({"pattern": "x", "fixed": "maybe"}), "`fixed`"),
        ("grep", json!({"pattern": "(unclosed"}), "invalid pattern"),
        ("grep", json!({"pattern": "a{2,1}"}), "invalid pattern"),
        ("refs", json!({"targets": ["dev_read"], "mode": 7}), "`mode`"),
        ("refs", json!({"targets": ["dev_read"], "mode": ["callers"]}), "`mode`"),
    ];
    for (tool, arguments, expect) in cases {
        let r = s.call(tool, arguments.clone());
        assert_eq!(r["isError"], true, "{tool} {arguments}: {r}");
        assert!(text(&r).contains(expect), "{tool} {arguments}: expected `{expect}` in `{}`", text(&r));
        assert_eq!(r["resultType"], "complete");
    }
    // Lenient where the intent is clear: a single string, the singular name, numbers as text.
    let r = s.call("search", json!({"queries": "dev_read"}));
    assert_eq!(r["isError"], false, "{r}");
    let r = s.call("search", json!({"query": "dev_read", "budget": "300"}));
    assert_eq!(r["isError"], false, "{r}");
    // An unknown symbol is an answer, not an error.
    let r = s.call("show", json!({"targets": ["no_such_symbol_anywhere"]}));
    assert_eq!(r["isError"], false, "{r}");
}

#[test]
fn subscription_stream_is_acknowledged_and_closed_gracefully() {
    let repo = Repo::new("listen");
    let mut s = Session::in_repo(&repo);
    let listen = |id: Value| json!({"jsonrpc": "2.0", "id": id, "method": "subscriptions/listen", "params": {"_meta": modern_meta(), "notifications": {"toolsListChanged": true, "resourcesListChanged": true}}});
    s.send(&listen(json!("sub-a")));
    let ack = s.recv();
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged", "{ack}");
    assert!(ack.get("id").is_none(), "the acknowledgement is a notification");
    assert_eq!(ack["params"]["_meta"][META_SUB], "sub-a");
    assert_eq!(ack["params"]["notifications"], json!({}), "nothing here ever changes, so nothing is agreed to");
    // The stream stays open: no response yet, and other requests are served meanwhile.
    assert!(s.next_line(Duration::from_millis(300)).is_none(), "a listen request is not answered while it is open");
    assert!(s.modern("tools/list", Value::Null)["result"]["tools"].is_array());

    // A second stream, cancelled by the client: it gets no completion.
    s.send(&listen(json!(77)));
    assert_eq!(s.recv()["params"]["_meta"][META_SUB], 77);
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 77, "reason": "test"}}));

    // A listen request without its filter is malformed.
    s.send(&json!({"jsonrpc": "2.0", "id": 5, "method": "subscriptions/listen", "params": {"_meta": modern_meta()}}));
    assert_eq!(error_code(&s.recv()), -32602);

    let done = s.finish();
    assert_eq!(done.code, Some(0));
    assert_eq!(done.rest.len(), 1, "exactly the open stream is completed: {:?}", done.rest);
    let closing: Value = serde_json::from_str(&done.rest[0]).unwrap();
    assert_eq!(closing["id"], "sub-a");
    assert_eq!(closing["result"]["resultType"], "complete");
    assert_eq!(closing["result"]["_meta"][META_SUB], "sub-a");
}

#[test]
fn progress_tokens_and_unknown_metadata_are_tolerated() {
    let repo = Repo::new("progress");
    let mut s = Session::in_repo(&repo);
    let mut meta = modern_meta();
    meta["progressToken"] = json!("p-1");
    meta["traceparent"] = json!("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01");
    meta["com.example/anything"] = json!({"x": 1});
    s.send(&json!({"jsonrpc": "2.0", "id": "p", "method": "tools/call", "params": {"_meta": meta, "name": "search", "arguments": {"queries": ["dev_read"]}}}));
    let reply = s.recv();
    assert_eq!(reply["id"], "p", "no notification precedes the result: {reply}");
    assert_eq!(reply["result"]["isError"], false);
}

#[test]
fn an_answer_never_outgrows_its_budget() {
    // `budget` is a promise of the tool definitions. A query that matches nothing is echoed back
    // in the answer, so a very long argument must not come back at full length.
    let repo = Repo::new("budget");
    let mut s = Session::in_repo(&repo);
    let long = "x".repeat(100_000);
    let cases = [
        ("search", json!({"queries": [long]})),
        ("show", json!({"targets": [long]})),
        ("refs", json!({"targets": [long]})),
        ("refs", json!({"targets": [long], "mode": "callers"})),
        ("path", json!({"from": long, "to": "main"})),
        ("grep", json!({"pattern": long})),
    ];
    for (tool, arguments) in cases {
        let r = s.call(tool, arguments.clone());
        let answer = text(&r);
        assert!(answer.len() <= 10_000, "{tool}: {} bytes came back at the default budget of 1500 tokens", answer.len());
        assert!(answer.contains("answer cut"), "{tool}: the cut is announced: …{}", &answer[answer.len().saturating_sub(120)..]);
        let mut big = arguments;
        big["budget"] = json!(8000);
        let r = s.call(tool, big);
        assert!(text(&r).len() <= 40_000, "{tool}: {} bytes at the largest budget", text(&r).len());
    }
    // The cut falls on a character boundary and the answer stays valid text.
    let r = s.call("search", json!({"queries": ["é😀".repeat(20_000)]}));
    assert!(text(&r).len() <= 10_000 && text(&r).contains("answer cut"));
    // An ordinary answer is not touched.
    let r = s.call("show", json!({"targets": ["src/big.c"], "budget": 8000}));
    assert!(!text(&r).contains("answer cut"), "{}", text(&r));
    assert!(text(&r).len() > 10_000, "the largest budget still returns a long listing: {}", text(&r).len());
}

#[test]
fn a_repeated_subscription_id_is_one_subscription() {
    let repo = Repo::new("listen-twice");
    let mut s = Session::in_repo(&repo);
    let listen = json!({"jsonrpc": "2.0", "id": "dup", "method": "subscriptions/listen", "params": {"_meta": modern_meta(), "notifications": {"toolsListChanged": true}}});
    for _ in 0..3 {
        s.send(&listen);
        assert_eq!(s.recv()["params"]["_meta"][META_SUB], "dup");
    }
    let done = s.finish();
    assert_eq!(done.code, Some(0));
    assert_eq!(done.rest.len(), 1, "one id, one completion: {:?}", done.rest);
}

#[test]
fn subscriptions_are_bounded() {
    // A client that keeps opening streams must not grow the server without limit.
    let repo = Repo::new("listen-many");
    let mut s = Session::in_repo(&repo);
    for i in 0..1024 {
        s.send(&json!({"jsonrpc": "2.0", "id": i, "method": "subscriptions/listen", "params": {"_meta": modern_meta(), "notifications": {}}}));
        assert_eq!(s.recv()["method"], "notifications/subscriptions/acknowledged", "stream {i}");
    }
    s.send(&json!({"jsonrpc": "2.0", "id": "one-too-many", "method": "subscriptions/listen", "params": {"_meta": modern_meta(), "notifications": {}}}));
    let reply = s.recv();
    assert_eq!(reply["id"], "one-too-many");
    assert_eq!(error_code(&reply), -32603, "{reply}");
    // Cancelling one makes room again.
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 5}}));
    s.send(&json!({"jsonrpc": "2.0", "id": "fits", "method": "subscriptions/listen", "params": {"_meta": modern_meta(), "notifications": {}}}));
    assert_eq!(s.recv()["params"]["_meta"][META_SUB], "fits");
    let done = s.finish();
    assert_eq!(done.rest.len(), 1024, "every open stream is completed at shutdown");
}

#[test]
fn initialize_with_a_valid_stateless_envelope_is_not_a_handshake() {
    // A request that carries a valid envelope of this revision is a request of this revision,
    // whatever its method: `initialize` does not exist here. (The official SDK's serving entry
    // classifies the opening message the same way.)
    let repo = Repo::new("init-modern");
    let mut s = Session::in_repo(&repo);
    let reply = s.modern("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}));
    assert_eq!(error_code(&reply), -32601, "{reply}");
    // With an envelope that is not valid for this revision it remains the legacy handshake.
    for meta in [json!({META_VERSION: "2025-11-25", META_CAPS: {}}), json!({META_VERSION: MODERN}), json!({"progressToken": 1})] {
        s.send(&json!({"jsonrpc": "2.0", "id": "h", "method": "initialize", "params": {"_meta": meta, "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}}));
        let reply = s.recv();
        assert_eq!(reply["result"]["protocolVersion"], "2025-11-25", "{reply}");
    }
}

#[test]
fn the_opening_of_claude_code_is_served() {
    // What Claude Code 2.1.322 (its SDK 2.0 runtime) writes when it connects, recorded from
    // `claude mcp list` on 2026-11-13: a discovery probe, then the tool list, both stateless.
    let repo = Repo::new("claude-code");
    let mut s = Session::in_repo(&repo);
    let meta = json!({
        META_VERSION: MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "claude-code", "title": "Claude Code", "version": "2.1.322", "description": "Anthropic's agentic coding tool", "websiteUrl": "https://claude.com/claude-code"},
        META_CAPS: {"roots": {"listChanged": true}, "elicitation": {"form": {}, "url": {}}}
    });
    s.send(&json!({"jsonrpc": "2.0", "id": "server-discover-probe-1", "method": "server/discover", "params": {"_meta": meta}}));
    let probe = s.recv();
    assert_eq!(probe["id"], "server-discover-probe-1");
    assert_modern_result(&probe);
    // The official client accepts the probe only when these are present and well-typed.
    let r = &probe["result"];
    assert!(r["supportedVersions"].as_array().unwrap().contains(&json!(MODERN)));
    assert!(r["capabilities"].is_object() && r["ttlMs"].is_u64() && r["cacheScope"].is_string());
    for key in r.as_object().unwrap().keys() {
        assert!(
            ["supportedVersions", "capabilities", "instructions", "ttlMs", "cacheScope", "resultType", "_meta"].contains(&key.as_str()),
            "unexpected key {key} in a DiscoverResult"
        );
    }
    s.send(&json!({"method": "tools/list", "jsonrpc": "2.0", "id": 0, "params": {"_meta": meta}}));
    let list = s.recv();
    assert_eq!(list["id"], 0);
    assert_modern_result(&list);
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 5);
    // A call in the same shape; the result is a CallToolResult and nothing else.
    s.send(&json!({"method": "tools/call", "jsonrpc": "2.0", "id": 1, "params": {"_meta": meta, "name": "search", "arguments": {"queries": ["dev_read"]}}}));
    let call = s.recv();
    assert_modern_result(&call);
    for key in call["result"].as_object().unwrap().keys() {
        assert!(["content", "isError", "resultType", "_meta"].contains(&key.as_str()), "unexpected key {key} in a CallToolResult");
    }
    assert_eq!(s.seen.len(), 3, "one line per request, nothing unsolicited");
}

#[test]
fn the_instructions_say_what_the_tools_leave_out() {
    // References, callers and grep skip test code unless `tests` is set (the command line has
    // `--tests`), and those answers do not mention what they skipped. The instructions
    // therefore have to, or an agent concludes that nothing in the tests uses a function.
    let repo = Repo::new("tests-left-out");
    write(
        &repo.path("tests/test_io.c"),
        "#include \"../src/io.h\"\n\nint test_reads_twice(struct dev *d, char *buf)\n{\n    /* only-in-the-test-file */\n    return dev_read(d, buf, 1) + dev_read(d, buf, 2);\n}\n",
    );
    let mut s = Session::in_repo(&repo);
    let r = s.call("refs", json!({"targets": ["dev_read"]}));
    assert!(text(&r).contains("src/main.c") && !text(&r).contains("tests/test_io.c"), "{}", text(&r));
    let r = s.call("refs", json!({"targets": ["dev_read"], "mode": "callers", "depth": 2}));
    assert!(text(&r).contains("copy_all") && !text(&r).contains("test_reads_twice"), "{}", text(&r));
    let r = s.call("grep", json!({"pattern": "only-in-the-test-file", "fixed": true}));
    assert!(text(&r).starts_with("no matches") && text(&r).contains("`tests: true`"), "{}", text(&r));
    // Asked for, the test code is there.
    let r = s.call("refs", json!({"targets": ["dev_read"], "tests": true}));
    assert!(text(&r).contains("src/main.c") && text(&r).contains("tests/test_io.c"), "{}", text(&r));
    let r = s.call("refs", json!({"targets": ["dev_read"], "mode": "callers", "depth": 2, "tests": true}));
    assert!(text(&r).contains("test_reads_twice"), "{}", text(&r));
    let r = s.call("grep", json!({"pattern": "only-in-the-test-file", "fixed": true, "tests": true}));
    assert!(text(&r).contains("tests/test_io.c"), "{}", text(&r));
    // The code itself is reachable: by name, and as source.
    let r = s.call("search", json!({"queries": ["test_reads_twice"]}));
    assert!(text(&r).contains("tests/test_io.c:3"), "{}", text(&r));
    let instructions = s.modern("server/discover", Value::Null)["result"]["instructions"].as_str().unwrap().to_string();
    assert!(instructions.contains("leave out test code"), "{instructions}");
    // Codex keeps the first 512 characters: what the server is for must be complete within them.
    let head: String = instructions.chars().take(512).collect();
    assert!(head.contains("Use it before grep and file reads") && head.contains("`search` is the entry point"), "{head}");
}

/// Free text is an open question: the answer carries the source of the functions that make a
/// file a candidate, so that it can be read instead of followed up one function at a time. A
/// caller that names a budget, small or large, gets the ranking alone in that size, and a
/// symbol is answered as ever.
#[test]
fn free_text_is_answered_with_the_code_of_the_candidates() {
    let repo = Repo::new("free-text");
    let mut s = Session::in_repo(&repo);
    let question = "the copy stops after a short write to the device";

    let r = s.call("search", json!({"queries": [question]}));
    let t = text(&r);
    assert!(t.contains("The source of the functions that make the first"), "{t}");
    assert!(t.contains("src/io.c") && t.contains("log_msg(\"dev_write: %d bytes\", count);"), "the body is part of the answer: {t}");

    for budget in [600, 6000] {
        let r = s.call("search", json!({"queries": [question], "budget": budget}));
        let t = text(&r);
        assert!(t.contains("candidate files; top") && !t.contains("log_msg("), "an answer whose size was named is the ranking: {t}");
    }

    let r = s.call("search", json!({"queries": ["dev_write"]}));
    assert!(text(&r).contains("src/io.c:11") && !text(&r).contains("candidate files"), "{}", text(&r));
}

/// A call with several queries is answered query by query: a name asked for after an open
/// question still gets its answer, and the open questions of the call are one question asked
/// in several ways, ranked and answered once.
#[test]
fn every_query_of_a_call_is_answered_and_its_open_questions_together() {
    let repo = Repo::new("several-queries");
    let mut s = Session::in_repo(&repo);

    let r = s.call("search", json!({"queries": ["the copy stops after a short write", "bytes written to the device", "dev_read", "dev_write"]}));
    let t = text(&r);
    assert!(t.contains("2 questions, answered together"), "{t}");
    assert_eq!(t.matches("candidate files").count(), 1, "one ranking for both questions: {t}");
    assert!(t.contains("src/io.c:4") && t.contains("src/io.c:11"), "each name has its answer after it: {t}");

    // A budget the caller names is the room of the whole call, and each query has a part of it.
    let r = s.call("search", json!({"queries": ["dev_read", "dev_write"], "budget": 300}));
    let t = text(&r);
    assert!(t.contains("src/io.c:4") && t.contains("src/io.c:11"), "{t}");

    // One open question in a call is answered as it is alone.
    let r = s.call("search", json!({"queries": ["dev_read", "the copy stops after a short write"]}));
    let t = text(&r);
    assert!(t.contains("src/io.c:4") && t.contains("candidate files") && !t.contains("answered together"), "{t}");
}

/// A text that stands in the code exactly as it was typed, in more than one place, is looked
/// for where it stands: the places, each with its function. It is not an open question.
#[test]
fn a_text_that_stands_in_the_code_is_answered_with_its_places() {
    let repo = Repo::new("as-typed");
    // The same message in a second file: no one function writes it.
    write(&repo.path("src/note.c"), "#include \"io.h\"\n\nvoid note(int count)\n{\n    log_msg(\"dev_write: %d bytes\", count);\n}\n");
    let mut s = Session::in_repo(&repo);

    let r = s.call("search", json!({"queries": ["dev_write: %d bytes"]}));
    let t = text(&r);
    assert!(t.contains("src/io.c") && t.contains("src/note.c") && !t.contains("candidate files"), "both places: {t}");

    // A declaration, typed as it is written.
    let r = s.call("search", json!({"queries": ["struct dev *d, char *buf, int count"]}));
    let t = text(&r);
    assert!(t.contains("src/io.h") && t.contains("src/io.c") && !t.contains("candidate files"), "{t}");

    // Words that stand nowhere as typed are still an open question, and so are two plain
    // words that a comment happens to hold ("the device").
    for q in ["bytes written by the copy loop", "the device"] {
        let r = s.call("search", json!({"queries": [q]}));
        assert!(text(&r).contains("candidate files"), "{}", text(&r));
    }
}

/// What models put into one query besides a question: several names at once, and a piece of
/// code. Neither is a text to rank files by.
#[test]
fn several_names_and_a_piece_of_code_are_not_ranked_as_words() {
    let repo = Repo::new("query-shapes");
    let mut s = Session::in_repo(&repo);

    let r = s.call("search", json!({"queries": ["dev_read dev_write"]}));
    let t = text(&r);
    assert!(t.contains("src/io.c:4") && t.contains("src/io.c:11") && !t.contains("candidate files"), "each name is answered: {t}");

    let r = s.call("search", json!({"queries": ["d->ops->write(d, buf, count)"]}));
    let t = text(&r);
    assert!(t.contains("src/io.c:14") && t.contains("dev_write") && !t.contains("candidate files"), "the code is found where it is written: {t}");

    // With other spacing than the source has, and a file named with it.
    let r = s.call("search", json!({"queries": ["io.c d->ops->read( d,buf,count )"]}));
    assert!(text(&r).contains("src/io.c:8"), "{}", text(&r));

    // Code that is written nowhere falls back to a ranking of its words.
    let r = s.call("search", json!({"queries": ["d->ops->flush(d) after the write"], "budget": 600}));
    assert!(text(&r).contains("candidate files"), "{}", text(&r));
}
