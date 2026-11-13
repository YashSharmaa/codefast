//! The handshake era of the MCP server: `initialize`, then plain requests, for clients that
//! speak revision 2025-11-25 or earlier and have no way to fall forward.

#[path = "packaging/support.rs"]
mod support;

use serde_json::{Value, json};
use std::time::Duration;
use support::*;

fn initialize(s: &mut Session, version: &str) -> Value {
    s.legacy(
        "initialize",
        json!({"protocolVersion": version, "capabilities": {"roots": {"listChanged": true}}, "clientInfo": {"name": "legacy-client", "version": "1.0"}}),
    )
}

#[test]
fn initialize_negotiates_every_handshake_revision() {
    let repo = Repo::new("negotiate");
    for version in ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"] {
        let mut s = Session::in_repo(&repo);
        let reply = initialize(&mut s, version);
        let r = &reply["result"];
        assert_eq!(r["protocolVersion"], version, "a supported revision is echoed: {reply}");
        assert!(r["capabilities"]["tools"].is_object());
        assert_eq!(r["serverInfo"]["name"], "codefast");
        assert_eq!(r["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(r["instructions"].as_str().is_some_and(|i| i.starts_with("codefast")));
        // Nothing from the stateless revision leaks into a legacy result.
        for key in ["resultType", "ttlMs", "cacheScope", "supportedVersions", "_meta"] {
            assert!(r.get(key).is_none(), "{key} does not belong in an initialize result: {reply}");
        }
    }
}

#[test]
fn unknown_or_newer_versions_get_the_newest_handshake_revision() {
    let repo = Repo::new("fallback");
    for requested in ["1999-01-01", "2026-07-28", "2030-12-31", ""] {
        let mut s = Session::in_repo(&repo);
        let reply = initialize(&mut s, requested);
        assert_eq!(reply["result"]["protocolVersion"], "2025-11-25", "requested {requested:?}: {reply}");
    }
    // A client that sends no version at all is still answered.
    let mut s = Session::in_repo(&repo);
    let reply = s.legacy("initialize", json!({}));
    assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn handshake_then_list_and_call() {
    let repo = Repo::new("handshake");
    let mut s = Session::in_repo(&repo);
    initialize(&mut s, "2025-06-18");
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    assert_eq!(s.legacy("ping", Value::Null)["result"], json!({}), "ping exists in the handshake era");

    let list = s.legacy("tools/list", json!({}));
    let r = &list["result"];
    assert_eq!(r["tools"].as_array().map(|t| t.len()), Some(5));
    assert_eq!(r.as_object().unwrap().len(), 1, "a legacy list result is just the tools: {list}");

    let call = s.legacy("tools/call", json!({"name": "search", "arguments": {"queries": ["dev_write"]}}));
    let r = &call["result"];
    assert_eq!(r["isError"], false);
    assert!(text(r).contains("src/io.c:11"), "{}", text(r));
    assert!(r.get("resultType").is_none() && r.get("_meta").is_none(), "{call}");

    let call = s.legacy("tools/call", json!({"name": "grep", "arguments": {}}));
    assert_eq!(call["result"]["isError"], true);
    assert_eq!(error_code(&s.legacy("tools/call", json!({"name": "nope"}))), -32602);

    let done = s.finish();
    assert_eq!(done.code, Some(0));
    assert!(done.rest.is_empty(), "nothing is sent after the last response: {:?}", done.rest);
}

#[test]
fn notifications_are_never_answered() {
    let repo = Repo::new("notify");
    let mut s = Session::in_repo(&repo);
    initialize(&mut s, "2025-11-25");
    for method in ["notifications/initialized", "notifications/cancelled", "notifications/roots/list_changed", "notifications/progress", "made/up"] {
        s.send(&json!({"jsonrpc": "2.0", "method": method, "params": {"requestId": 1, "progressToken": 1, "progress": 1}}));
    }
    // Even a notification with unusable params stays unanswered.
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": [1, 2, 3]}));
    assert!(s.next_line(Duration::from_millis(400)).is_none(), "a notification must not be answered");
    assert_eq!(s.legacy("ping", Value::Null)["result"], json!({}));
}

#[test]
fn requests_without_a_handshake_are_served_as_legacy() {
    // Scripts and some older clients skip `initialize`; they still get plain legacy results.
    let repo = Repo::new("nohandshake");
    let mut s = Session::in_repo(&repo);
    let list = s.legacy("tools/list", Value::Null);
    assert!(list["result"]["tools"].is_array());
    assert!(list["result"].get("resultType").is_none());
    let call = s.legacy("tools/call", json!({"name": "show", "arguments": {"targets": ["copy_all"]}}));
    assert!(text(&call["result"]).contains("src/main.c:3"), "{call}");
}

#[test]
fn clients_that_ignore_capabilities_get_empty_lists() {
    let repo = Repo::new("empties");
    let mut s = Session::in_repo(&repo);
    initialize(&mut s, "2024-11-05");
    assert_eq!(s.legacy("resources/list", Value::Null)["result"], json!({"resources": []}));
    assert_eq!(s.legacy("resources/templates/list", Value::Null)["result"], json!({"resourceTemplates": []}));
    assert_eq!(s.legacy("prompts/list", Value::Null)["result"], json!({"prompts": []}));
    assert_eq!(error_code(&s.legacy("resources/read", json!({"uri": "file:///etc/passwd"}))), -32601);
    assert_eq!(error_code(&s.legacy("prompts/get", json!({"name": "x"}))), -32601);
    assert_eq!(error_code(&s.legacy("logging/setLevel", json!({"level": "debug"}))), -32601);
    assert_eq!(error_code(&s.legacy("sampling/createMessage", Value::Null)), -32601);
}

#[test]
fn batches_of_revision_2025_03_26_are_answered_as_batches() {
    let repo = Repo::new("batch");
    let mut s = Session::in_repo(&repo);
    initialize(&mut s, "2025-03-26");
    s.send(&json!([
        {"jsonrpc": "2.0", "id": "a", "method": "ping"},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": "b", "method": "tools/call", "params": {"name": "search", "arguments": {"queries": ["dev_read"]}}},
        {"jsonrpc": "2.0", "id": "c", "method": "unknown/method"},
        17
    ]));
    let reply = s.recv();
    let items = reply.as_array().expect("a batch is answered with one array");
    assert_eq!(items.len(), 4, "one response per request, none for the notification: {reply}");
    assert_eq!(items[0]["id"], "a");
    assert_eq!(items[1]["id"], "b");
    assert!(text(&items[1]["result"]).contains("src/io.c:4"));
    assert_eq!(error_code(&items[2]), -32601);
    assert_eq!(error_code(&items[3]), -32600);
    // A batch of notifications alone produces no output at all.
    s.send(&json!([{"jsonrpc": "2.0", "method": "notifications/initialized"}]));
    assert!(s.next_line(Duration::from_millis(300)).is_none());
    // An empty batch is an invalid request.
    s.send_raw("[]");
    assert_eq!(error_code(&s.recv()), -32600);
}

#[test]
fn the_server_never_sends_requests() {
    // A client that advertises roots, sampling and elicitation must not be asked for any of them:
    // everything the server writes is a response or (for an open subscription) a notification.
    let repo = Repo::new("norequests");
    let mut s = Session::in_repo(&repo);
    s.legacy("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {"roots": {"listChanged": true}, "sampling": {}, "elicitation": {}}, "clientInfo": {"name": "c", "version": "1"}}));
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    s.legacy("tools/list", Value::Null);
    s.legacy("tools/call", json!({"name": "search", "arguments": {"queries": ["dev_read"]}}));
    assert_eq!(s.seen.len(), 3);
    for line in &s.seen {
        let msg: Value = serde_json::from_str(line).unwrap();
        assert!(msg.get("method").is_none(), "the server wrote a request or notification: {line}");
        assert!(msg.get("id").is_some() && msg.get("result").is_some(), "not a response: {line}");
    }
    let done = s.finish();
    assert_eq!(done.code, Some(0));
    assert!(done.rest.is_empty());
}

#[test]
fn the_opening_of_codex_is_served() {
    // What Codex CLI 0.164.0 writes (recorded from its app server on 2026-11-13): the 2025-06-18
    // handshake, lists with a progress token, resource lists it asks for although the server
    // declares no resources, and calls that carry its own keys in `_meta`.
    let repo = Repo::new("codex");
    let mut s = Session::in_repo(&repo);
    s.send(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {"experimental": {"codex/auth-change": {}}, "elicitation": {"form": {}, "url": {}}}, "clientInfo": {"name": "codex-mcp-client", "title": "Codex", "version": "0.164.0"}}}));
    let init = s.recv();
    assert_eq!(init["id"], 0);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    for key in init["result"].as_object().unwrap().keys() {
        assert!(["protocolVersion", "capabilities", "serverInfo", "instructions"].contains(&key.as_str()), "unexpected key {key} in an InitializeResult");
    }
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    s.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": {"progressToken": 0}}}));
    let list = s.recv();
    assert_eq!(list["id"], 1);
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 5);
    assert!(list["result"].get("resultType").is_none(), "a progress token does not make a request stateless");
    // Sent out of order, answered in the order received.
    s.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list", "params": {"_meta": {"progressToken": 2}}}));
    s.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "resources/templates/list", "params": {"_meta": {"progressToken": 1}}}));
    let (first, second) = (s.recv(), s.recv());
    assert_eq!((first["id"].as_i64(), second["id"].as_i64()), (Some(3), Some(2)));
    assert_eq!(first["result"], json!({"resources": []}));
    assert_eq!(second["result"], json!({"resourceTemplates": []}));
    s.send(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"_meta": {"threadId": "01a1d17a-8e0a-7fc3-b915-648430be6455", "progressToken": 3}, "name": "refs", "arguments": {"targets": ["dev_read"], "mode": "callers"}}}));
    let call = s.recv();
    assert_eq!(call["result"]["isError"], false, "{call}");
    assert!(text(&call["result"]).contains("copy_all"), "{}", text(&call["result"]));
    for key in call["result"].as_object().unwrap().keys() {
        assert!(["content", "isError"].contains(&key.as_str()), "unexpected key {key} in a legacy CallToolResult");
    }
    assert_eq!(s.seen.len(), 5, "no progress notification, no request of the server's own");
}

#[test]
fn the_legacy_opening_of_claude_code_is_served() {
    // Claude Code 2.1.322 with MCP_PROTOCOL_NEGOTIATION=legacy or MCP_SDK_GENERATION=v1.
    let repo = Repo::new("claude-legacy");
    let mut s = Session::in_repo(&repo);
    s.send(&json!({"method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {"roots": {"listChanged": true}, "elicitation": {"form": {}, "url": {}}}, "clientInfo": {"name": "claude-code", "title": "Claude Code", "version": "2.1.322", "description": "Anthropic's agentic coding tool", "websiteUrl": "https://claude.com/claude-code"}}, "jsonrpc": "2.0", "id": 0}));
    assert_eq!(s.recv()["result"]["protocolVersion"], "2025-11-25");
    s.send(&json!({"method": "notifications/initialized", "jsonrpc": "2.0"}));
    s.send(&json!({"method": "tools/list", "jsonrpc": "2.0", "id": 1}));
    let list = s.recv();
    assert_eq!(list["result"].as_object().unwrap().len(), 1, "{list}");
    // The client advertises roots; it is never asked for them.
    assert_eq!(s.seen.len(), 2);
}
