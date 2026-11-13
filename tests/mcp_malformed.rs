//! Malformed and hostile input: the server answers with the right JSON-RPC error, keeps
//! serving, writes nothing but protocol messages to stdout, and exits when stdin closes.

#[path = "packaging/support.rs"]
mod support;

use serde_json::{Value, json};
use std::time::Duration;
use support::*;

/// The server still answers a normal request.
fn assert_alive(s: &mut Session) {
    let reply = s.legacy("ping", Value::Null);
    assert_eq!(reply["result"], json!({}), "the server stopped serving: {reply}");
}

#[test]
fn invalid_json_is_a_parse_error_without_an_id() {
    let repo = Repo::new("parse");
    let mut s = Session::in_repo(&repo);
    for line in ["not json at all", "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":", "{'single': 'quotes'}", "{\"a\":1}{\"b\":2}", "\u{feff}{}"] {
        s.send_raw(line);
        let reply = s.recv();
        assert_eq!(error_code(&reply), -32700, "{line}: {reply}");
        assert!(reply.get("id").is_none(), "an unreadable request has no id to echo: {reply}");
        assert_eq!(reply["jsonrpc"], "2.0");
    }
    assert_alive(&mut s);
}

#[test]
fn invalid_utf8_and_binary_noise_do_not_kill_the_server() {
    let repo = Repo::new("utf8");
    let mut s = Session::in_repo(&repo);
    s.send_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"\xff\xfe\"}\n");
    assert_eq!(error_code(&s.recv()), -32700);
    s.send_bytes(&[0x00, 0x01, 0x02, 0xc3, 0x28, b'\n']);
    assert_eq!(error_code(&s.recv()), -32700);
    assert_alive(&mut s);
}

#[test]
fn values_that_are_not_requests_are_invalid_requests() {
    let repo = Repo::new("invalid");
    let mut s = Session::in_repo(&repo);
    for line in ["42", "\"hello\"", "null", "true", "[]", "{}", "{\"jsonrpc\":\"2.0\",\"id\":3}", "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":17}"] {
        s.send_raw(line);
        let reply = s.recv();
        assert_eq!(error_code(&reply), -32600, "{line}: {reply}");
    }
    // An id that is not a string or a number cannot be echoed, so it is left out.
    for id in [json!(null), json!(true), json!({"a": 1}), json!([1])] {
        s.send(&json!({"jsonrpc": "2.0", "id": id, "method": "ping"}));
        let reply = s.recv();
        assert_eq!(error_code(&reply), -32600, "{reply}");
        assert!(reply.get("id").is_none(), "{reply}");
    }
    // A wrong protocol marker is rejected with the id it came with.
    s.send(&json!({"jsonrpc": "1.0", "id": 8, "method": "ping"}));
    let reply = s.recv();
    assert_eq!(error_code(&reply), -32600);
    assert_eq!(reply["id"], 8);
    // Params must be an object.
    s.send(&json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": ["search", "x"]}));
    let reply = s.recv();
    assert_eq!(error_code(&reply), -32602);
    assert_eq!(reply["id"], 9);
    assert_alive(&mut s);
}

#[test]
fn string_and_numeric_ids_are_echoed_exactly() {
    let repo = Repo::new("ids");
    let mut s = Session::in_repo(&repo);
    for id in [json!(0), json!(-7), json!(9007199254740993u64), json!("abc"), json!(""), json!("with \"quotes\" and \n newline"), json!(1.5)] {
        s.send(&json!({"jsonrpc": "2.0", "id": id, "method": "ping"}));
        let reply = s.recv();
        assert_eq!(reply["id"], id, "{reply}");
    }
}

#[test]
fn responses_from_the_client_are_ignored() {
    // The server never sends a request, so any response is unsolicited.
    let repo = Repo::new("responses");
    let mut s = Session::in_repo(&repo);
    s.send(&json!({"jsonrpc": "2.0", "id": 1, "result": {"roots": []}}));
    s.send(&json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -32601, "message": "x"}}));
    assert!(s.next_line(Duration::from_millis(300)).is_none());
    assert_alive(&mut s);
}

#[test]
fn blank_lines_crlf_and_a_missing_final_newline_are_fine() {
    let repo = Repo::new("framing");
    let mut s = Session::in_repo(&repo);
    s.send_bytes(b"\n\n   \n\t\n");
    s.send_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\r\n");
    assert_eq!(s.recv()["id"], 1);
    // Two messages in one write, and the last one without a newline before stdin closes.
    s.send_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}");
    assert_eq!(s.recv()["id"], 2);
    let done = s.finish();
    assert_eq!(done.code, Some(0));
    assert_eq!(done.rest.len(), 1, "{:?}", done.rest);
    assert_eq!(serde_json::from_str::<Value>(&done.rest[0]).unwrap()["id"], 3);
}

#[test]
fn deeply_nested_and_oversized_messages_are_rejected_without_harm() {
    let repo = Repo::new("large");
    let mut s = Session::in_repo(&repo);
    // Nesting beyond the JSON parser's recursion limit.
    let deep = format!("{}{}", "[".repeat(50_000), "]".repeat(50_000));
    s.send_raw(&deep);
    assert_eq!(error_code(&s.recv()), -32700);
    // A large but legitimate message is served.
    let big = "x".repeat(2 << 20);
    s.send(&json!({"jsonrpc": "2.0", "id": "big", "method": "ping", "params": {"_meta": {"com.example/padding": big}}}));
    let reply = s.recv();
    assert_eq!(reply["id"], "big");
    assert_eq!(reply["result"], json!({}), "a 2 MiB request is still a request");
    // A message longer than the cap is skipped, reported once, and does not exhaust memory.
    let chunk = vec![b'a'; 1 << 20];
    s.send_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"");
    for _ in 0..17 {
        s.send_bytes(&chunk);
    }
    s.send_bytes(b"\"}\n");
    let reply = s.recv();
    assert_eq!(error_code(&reply), -32600, "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("16 MiB"));
    assert_alive(&mut s);
}

#[test]
fn stdout_carries_only_protocol_messages() {
    // A whole conversation, including the first call that builds the index: every line on stdout
    // is one JSON-RPC message, and the build's progress never appears there.
    let repo = Repo::new("stdout");
    let mut s = Session::in_repo(&repo);
    s.send_raw("garbage");
    s.recv();
    s.legacy("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}));
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    s.legacy("tools/list", Value::Null);
    s.legacy("tools/call", json!({"name": "search", "arguments": {"queries": ["dev_read"]}}));
    s.modern("server/discover", Value::Null);
    s.call("grep", json!({"pattern": "(unclosed"}));
    s.call("show", json!({"targets": ["src/big.c"]}));
    s.legacy("tools/call", json!({"name": "search", "arguments": {"queries": ["x"], "root": "/definitely/not/here"}}));
    assert!(repo.path(".codefast").is_dir(), "the first call built the index");
    let seen = s.seen.clone();
    let done = s.finish();
    assert_eq!(done.code, Some(0));
    for line in seen.iter().chain(done.rest.iter()) {
        let msg: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"));
        assert_eq!(msg["jsonrpc"], "2.0", "{line}");
        assert!(msg.get("result").is_some() || msg.get("error").is_some(), "{line}");
        assert!(!line.contains('\r'), "no embedded line breaks");
    }
    assert_eq!(seen.len(), 8);
}

#[test]
fn the_server_exits_when_stdin_closes() {
    let repo = Repo::new("eof");
    // Immediately.
    let done = Session::in_repo(&repo).finish();
    assert_eq!(done.code, Some(0));
    assert!(done.rest.is_empty());
    // After work, with requests still queued behind the close: they are answered first.
    let mut s = Session::in_repo(&repo);
    s.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "search", "arguments": {"queries": ["dev_read"]}}}));
    s.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}));
    let done = s.finish();
    assert_eq!(done.code, Some(0));
    assert_eq!(done.rest.len(), 2, "queued requests are answered before exit: {:?}", done.rest);
}

#[test]
fn errors_do_not_repeat_the_request_at_length() {
    // An error message is "a concise single sentence", whatever was sent.
    let repo = Repo::new("echo");
    let mut s = Session::in_repo(&repo);
    let long = "m".repeat(200_000);
    s.send(&json!({"jsonrpc": "2.0", "id": 1, "method": long}));
    let reply = s.recv();
    assert_eq!(error_code(&reply), -32601);
    assert!(reply.to_string().len() < 400, "{} bytes", reply.to_string().len());
    s.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": long, "arguments": {}}}));
    let reply = s.recv();
    assert_eq!(error_code(&reply), -32602);
    assert!(reply.to_string().len() < 400, "{} bytes", reply.to_string().len());
    s.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {"_meta": {META_VERSION: long, META_CAPS: {}}}}));
    let reply = s.recv();
    assert_eq!(error_code(&reply), -32022);
    assert!(reply.to_string().len() < 400, "{} bytes", reply.to_string().len());
    assert_eq!(reply["error"]["data"]["supported"], json!([MODERN]));
    // Tool errors are held to the answer budget as well.
    let reply = s.legacy("tools/call", json!({"name": "refs", "arguments": {"targets": ["dev_read"], "mode": long}}));
    assert_eq!(reply["result"]["isError"], true);
    assert!(text(&reply["result"]).len() < 300, "{}", text(&reply["result"]).len());
    let reply = s.legacy("tools/call", json!({"name": "search", "arguments": {"queries": ["x"], "root": format!("/{long}")}}));
    assert_eq!(reply["result"]["isError"], true);
    assert!(text(&reply["result"]).len() < 1_000, "{}", text(&reply["result"]).len());
    assert_alive(&mut s);
}
