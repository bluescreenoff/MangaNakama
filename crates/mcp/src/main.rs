//! `mn-mcp` — MCP (Model Context Protocol) shim over the MangaNakama
//! automation socket (docs/AUTOMATION.md).
//!
//! Deliberately a SEPARATE binary: the app speaks its own tiny JSON-RPC
//! over TCP and knows nothing about MCP; this shim translates stdio-MCP on
//! one side to that socket on the other. No SDK — MCP's stdio transport is
//! newline-delimited JSON-RPC 2.0, ~a dozen methods of which a tools-only
//! server needs four. If MCP moves under us, this file moves, the app
//! does not.
//!
//! Wiring (Claude Code):
//!   claude mcp add manganakama -- <path>\mn-mcp.exe
//! The shim finds the running app via `automation.txt` beside its own exe
//! (ship it in `play/` next to `manganakama.exe`), or wherever
//! `--auto-file <path>` points. The app must have Preferences ▸ Performance
//! ▸ Automation server turned ON.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use serde_json::{Value, json};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut auto_file: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--auto-file" => auto_file = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                eprintln!("mn-mcp [--auto-file <path-to-automation.txt>]");
                return;
            }
            other => eprintln!("[mn-mcp] ignoring unknown arg {other}"),
        }
    }
    let auto_file = auto_file
        .or_else(|| {
            Some(
                std::env::current_exe()
                    .ok()?
                    .parent()?
                    .join("automation.txt"),
            )
        })
        .expect("an automation.txt location");

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut backend = Backend::new(auto_file);

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[mn-mcp] unparseable line from client: {e}");
                continue;
            }
        };
        // Notifications (no id) get no response — answering one is a
        // protocol violation that some clients treat as fatal.
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let Some(id) = id else { continue };
        let response = serve(method, msg.get("params").unwrap_or(&Value::Null), &mut backend);
        let frame = match response {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        if writeln!(out, "{frame}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
}

fn serve(method: &str, params: &Value, backend: &mut Backend) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            // Echo the client's protocol version — we have no version-
            // dependent behaviour, and echoing is what keeps both older
            // and newer clients happy.
            let ver = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18");
            Ok(json!({
                "protocolVersion": ver,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "manganakama", "version": env!("CARGO_PKG_VERSION")},
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_table()})),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or((-32602, "params.name required".to_owned()))?;
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let socket_method = TOOLS
                .iter()
                .find(|t| t.0 == name)
                .map(|t| t.1)
                .ok_or((-32602, format!("unknown tool {name}")))?;
            // A tool failure is a RESULT with isError, not a protocol
            // error — the model is supposed to read it and adapt.
            let (text, is_error) = match backend.call(socket_method, args) {
                Ok(v) => (v.to_string(), false),
                Err(e) => (e, true),
            };
            Ok(json!({
                "content": [{"type": "text", "text": text}],
                "isError": is_error,
            }))
        }
        other => Err((-32601, format!("unknown method {other}"))),
    }
}

/// (tool name, socket method, description). One row per socket method —
/// the shim adds nothing of its own, which is the whole point of a shim.
const TOOLS: &[(&str, &str, &str)] = &[
    (
        "doc_info",
        "doc.info",
        "Current document: path, page index, page count, canvas size [w,h] in px, dpi.",
    ),
    (
        "layers_list",
        "layers.list",
        "All layers of the current page, top of the stack first in paint terms: stable id, index, name, kind (raster/folder/fill/correction/frame/balloon/text), depth, visibility, opacity, which is active.",
    ),
    (
        "texts_list",
        "texts.list",
        "Text items of a text layer: stable id, content, vertical (JP columns), align, frame_align, font, size_pt, pos, size, rotation. Args: {layer: <layer id>}.",
    ),
    (
        "balloons_list",
        "balloons.list",
        "Balloon ids of a balloon layer. Args: {layer: <layer id>}.",
    ),
    (
        "layers_add_text",
        "layers.add_text",
        "Create a new (empty, undoable) text layer on the current page for lettering. Args: {name?}. Returns its stable id.",
    ),
    (
        "texts_patch",
        "texts.patch",
        "Batch-edit text items BY ID on one layer; one undo press for the whole batch. Args: {layer: <layer id>, items: [{id, text?, vertical?, align?: Leading|Center|Trailing, frame_align?: Near|Center|Far, font?, size_pt?, pos?, size?}]}. Changing text clears style runs; setting size turns auto_size off. Returns how many landed.",
    ),
    (
        "texts_add",
        "texts.add",
        "Add text items to a text layer; unspecified fields follow the page's template (JP vertical lettering by default). Args: {layer: <layer id>, items: [{text, pos?, size?, vertical?, align?, frame_align?, font?, size_pt?, color?, outline_px?}]}. Returns the minted ids.",
    ),
    (
        "texts_remove",
        "texts.remove",
        "Remove text items by id. Args: {layer: <layer id>, ids: [..]}. Returns how many were removed.",
    ),
    (
        "pages_select",
        "pages.select",
        "Switch the app to another page (pages are index-addressed). Args: {page: <index>}.",
    ),
    (
        "page_render",
        "page.render",
        "Render the current page's composite to a PNG on disk so you can look at it. Args: {path: <absolute .png path>}.",
    ),
    ("undo", "doc.undo", "Undo one step in the app, same as Ctrl+Z."),
    ("redo", "doc.redo", "Redo one step in the app."),
];

fn tool_table() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|(name, _, desc)| {
            json!({
                "name": name,
                "description": desc,
                // The app validates for real; a permissive schema keeps the
                // two out of drift and the errors come back readable.
                "inputSchema": {"type": "object", "additionalProperties": true},
            })
        })
        .collect()
}

/// The TCP side: connect lazily, auth once, one request per line. On any
/// IO failure re-read `automation.txt` and reconnect ONCE — the common
/// case is simply "the app restarted and the port moved".
struct Backend {
    auto_file: PathBuf,
    conn: Option<(BufReader<TcpStream>, TcpStream)>,
    next_id: u64,
}

impl Backend {
    fn new(auto_file: PathBuf) -> Self {
        Self {
            auto_file,
            conn: None,
            next_id: 1,
        }
    }

    fn connect(&mut self) -> Result<(), String> {
        let text = std::fs::read_to_string(&self.auto_file).map_err(|_| {
            format!(
                "no automation.txt at {} — is MangaNakama running with \
                 Preferences ▸ Performance ▸ Automation server ON?",
                self.auto_file.display()
            )
        })?;
        let mut port = None;
        let mut token = None;
        for line in text.lines() {
            match line.split_once('=') {
                Some(("port", v)) => port = v.trim().parse::<u16>().ok(),
                Some(("token", v)) => token = Some(v.trim().to_owned()),
                _ => {}
            }
        }
        let (Some(port), Some(token)) = (port, token) else {
            return Err("automation.txt is malformed".to_owned());
        };
        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("cannot reach the app on 127.0.0.1:{port}: {e}"))?;
        let read = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        self.conn = Some((read, stream));
        let hello = self.roundtrip(json!({
            "id": 0, "method": "auth", "params": {"token": token},
        }))?;
        if hello.get("error").is_some() {
            self.conn = None;
            return Err(format!("the app refused the token: {hello}"));
        }
        Ok(())
    }

    fn roundtrip(&mut self, frame: Value) -> Result<Value, String> {
        let Some((read, write)) = self.conn.as_mut() else {
            return Err("not connected".to_owned());
        };
        writeln!(write, "{frame}").map_err(|e| e.to_string())?;
        let mut line = String::new();
        match read.read_line(&mut line) {
            Ok(0) => Err("the app closed the connection".to_owned()),
            Ok(_) => serde_json::from_str(&line).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        for attempt in 0..2 {
            if self.conn.is_none() {
                self.connect()?;
            }
            let id = self.next_id;
            self.next_id += 1;
            let frame = json!({"id": id, "method": method, "params": params});
            match self.roundtrip(frame) {
                Ok(resp) => {
                    return match resp.get("error") {
                        Some(e) => Err(e.to_string()),
                        None => Ok(resp.get("result").cloned().unwrap_or(Value::Null)),
                    };
                }
                Err(e) if attempt == 0 => {
                    // Stale connection (app restarted): drop it and let the
                    // second attempt re-discover.
                    eprintln!("[mn-mcp] reconnecting: {e}");
                    self.conn = None;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!("the loop returns on both attempts");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool maps to a socket method exactly once, and the names stay
    /// snake_case (MCP tool-name rules).
    #[test]
    fn tool_table_is_sound() {
        for (name, method, desc) in TOOLS {
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name}"
            );
            assert!(method.contains('.') || method.starts_with("doc"), "{method}");
            assert!(!desc.is_empty());
        }
        let mut names: Vec<_> = TOOLS.iter().map(|t| t.0).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOOLS.len(), "duplicate tool names");
        assert_eq!(tool_table().len(), TOOLS.len());
    }

    /// The MCP surface: initialize echoes the client's protocol version,
    /// notifications never produce a frame (handled in main by the id
    /// gate), unknown tools error as -32602.
    #[test]
    fn serve_shapes() {
        let mut b = Backend::new(PathBuf::from("Z:/definitely/not/there.txt"));
        let init = serve(
            "initialize",
            &json!({"protocolVersion": "2199-01-01"}),
            &mut b,
        )
        .unwrap();
        assert_eq!(init["protocolVersion"], "2199-01-01");
        assert_eq!(init["serverInfo"]["name"], "manganakama");

        let listed = serve("tools/list", &Value::Null, &mut b).unwrap();
        assert!(listed["tools"].as_array().unwrap().len() >= 10);

        assert!(serve("tools/call", &json!({"name": "no_such"}), &mut b).is_err());
        assert!(serve("resources/list", &Value::Null, &mut b).is_err());

        // A tool call with no app reachable is an isError RESULT, not a
        // protocol error — the model reads the message and tells the user.
        let r = serve("tools/call", &json!({"name": "doc_info"}), &mut b).unwrap();
        assert_eq!(r["isError"], true);
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("automation.txt"), "{text}");
    }
}
