//! Plugin mock pour les tests d'intégration de l'hôte (#8).
//! Lit des requêtes JSON-RPC 2.0 ligne par ligne sur stdin et répond sur stdout.

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<serde_json::Value>(line) else {
            let _ = writeln!(
                out,
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32700,"message":"parse error"}}}}"#
            );
            continue;
        };
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let result = match method {
            "manifest.get" => serde_json::json!({
                "plugin": {
                    "id": "mock",
                    "name": "Mock Plugin",
                    "version": "0.1.0",
                    "category": "metadata",
                    "license": "AGPL-3.0-only"
                }
            }),
            "capabilities.list" => serde_json::json!(["metadata.enrich", "ping"]),
            "ping" => serde_json::json!({ "pong": true }),
            "echo" => req
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "boom" => {
                let _ = writeln!(
                    out,
                    r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32000,"message":"boom"}}}}"#
                );
                let _ = out.flush();
                continue;
            }
            other => {
                let _ = writeln!(
                    out,
                    r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32601,"message":"method not found: {other}"}}}}"#
                );
                let _ = out.flush();
                continue;
            }
        };
        let _ = writeln!(out, r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#);
        let _ = out.flush();
    }
}
