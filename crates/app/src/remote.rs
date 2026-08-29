//! Tier 3 automation (docs/AUTOMATION.md): a localhost JSON-RPC socket that
//! lets scripts — and the `mn-mcp` shim, and Claude Code directly — drive
//! the running app. Born from a CSP JP-typesetting session where per-item
//! align/direction/content could not be auto-actioned (archive TODO,
//! 2026-08-28).
//!
//! # Single-writer discipline
//!
//! Nothing here touches `App` or `Document` off the UI thread. A connection
//! thread parses one request, parks it in [`QUEUE`], wakes the message loop
//! with `PostMessageW(hwnd, MSG)`, and blocks on a reply channel. The
//! wndproc's `MSG` arm — UI thread, `&mut App` in hand like every other arm —
//! drains the queue through [`respond`], so a remote edit goes through the
//! same `cmd::dispatch` doors as a click, lands in the same undo history
//! (one `set_texts` commit = one Ctrl+Z), and repaints live while the artist
//! watches.
//!
//! Ordering with the UI's own command queue: the message loop drains
//! `app.cmds` after every dispatched message, so the queue is empty by the
//! time `MSG` arrives. The one exception is a modal dialog pumping messages
//! mid-`pump_commands`; [`respond`] answers `busy` when it sees a non-empty
//! queue instead of jumping ahead of it.
//!
//! # Trust boundary
//!
//! This socket is an arbitrary-command port into the owner's open document.
//! The defences, in order: OFF by default (`prefs.txt automation=`, honest-
//! bool rule — gibberish never reads as on); binds 127.0.0.1 only; speaks
//! raw newline-delimited JSON over TCP, which a browser page cannot (fetch
//! has no raw sockets, so localhost web pages cannot poke it); and every
//! connection must present the per-launch token from `automation.txt`
//! (beside the exe, readable only by the local user) before anything else.
//! Turning the preference off mid-session closes the gate immediately —
//! the port stays bound until restart but refuses auth and requests.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::app::App;
use crate::cmd::{self, AppCmd, BalloonPatch, TextPatch};

/// The wndproc message that says "remote requests are waiting". `WM_APP`
/// range = never collides with system or egui messages.
pub const MSG: u32 = WM_APP + 71;

/// How long a connection waits for the UI thread before answering `busy` on
/// its own. Long deliberately: a B4/600 filter can hold a frame for seconds,
/// and a premature timeout would orphan the reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// One parsed request parked for the UI thread.
pub struct Pending {
    pub req: Request,
    pub reply: SyncSender<String>,
}

static QUEUE: Mutex<VecDeque<Pending>> = Mutex::new(VecDeque::new());
/// The live gate: auth and requests both check it, so the Preferences
/// toggle takes effect on the next line, not the next restart.
static ENABLED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static PORT: AtomicU16 = AtomicU16::new(0);
static TOKEN: OnceLock<String> = OnceLock::new();

#[derive(serde::Deserialize)]
pub struct Request {
    /// Echoed back verbatim; absent = null, per JSON-RPC's notification
    /// blur — we always answer, a script that sent no id gets null.
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Session token. `RandomState` seeds each instance from OS entropy
/// (SipHash's two 64-bit keys), which is the standard library's only
/// randomness — no rand crate in the tree, and 128 unpredictable bits is
/// plenty for a localhost gate.
fn mint_token() -> String {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let mut s = String::with_capacity(32);
    for round in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(round);
        s.push_str(&format!("{:016x}", h.finish()));
    }
    s
}

/// `automation.txt` beside the exe — how clients (the MCP shim, scripts)
/// discover the ephemeral port and the token. Same beside-the-exe rule as
/// `prefs.txt`, and deleted whenever the server is off so a stale file
/// never advertises a dead or foreign port.
pub fn auto_file_path() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("automation.txt"))
}

pub fn remove_auto_file() {
    if let Some(p) = auto_file_path() {
        let _ = std::fs::remove_file(p);
    }
}

fn write_auto_file(port: u16, token: &str) {
    if let Some(p) = auto_file_path() {
        let _ = std::fs::write(p, format!("port={port}\ntoken={token}\n"));
    }
}

/// Open (or re-open) the gate. First call binds `127.0.0.1:0` and spawns the
/// accept loop; later calls — the Preferences toggle going off and on again —
/// reuse the bound port. Returns the port for the status line.
pub fn start(hwnd: isize) -> std::io::Result<u16> {
    let token = TOKEN.get_or_init(mint_token).clone();
    if STARTED.swap(true, Ordering::SeqCst) {
        let port = PORT.load(Ordering::SeqCst);
        ENABLED.store(true, Ordering::SeqCst);
        write_auto_file(port, &token);
        return Ok(port);
    }
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            STARTED.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };
    let port = listener.local_addr()?.port();
    PORT.store(port, Ordering::SeqCst);
    ENABLED.store(true, Ordering::SeqCst);
    write_auto_file(port, &token);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            std::thread::spawn(move || serve_connection(stream, hwnd));
        }
    });
    Ok(port)
}

/// Close the gate without unbinding (the accept thread has no clean kill;
/// refusing auth and requests is the part that matters).
pub fn stop() {
    ENABLED.store(false, Ordering::SeqCst);
    remove_auto_file();
}

/// The wndproc arm's half: everything parked since the last wake.
pub fn take_pending() -> Vec<Pending> {
    let mut q = QUEUE.lock().unwrap();
    q.drain(..).collect()
}

fn ok_response(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn err_response(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

fn serve_connection(stream: TcpStream, hwnd: isize) {
    let Ok(read) = stream.try_clone() else { return };
    let mut write = stream;
    let mut lines = BufReader::new(read).lines();

    // Handshake: the FIRST line must be `auth` with the token. Anything
    // else — including a valid request — is answered and the connection
    // dropped, so a port scanner learns nothing but "something JSON lives
    // here".
    let Some(Ok(first)) = lines.next() else { return };
    let authed = matches!(
        serde_json::from_str::<Request>(&first),
        Ok(r) if r.method == "auth"
            && r.params.get("token").and_then(Value::as_str) == TOKEN.get().map(String::as_str)
            && ENABLED.load(Ordering::SeqCst)
    );
    let hello = if authed {
        let id = serde_json::from_str::<Request>(&first).map(|r| r.id).unwrap_or(Value::Null);
        ok_response(
            &id,
            json!({"app": "manganakama", "version": env!("CARGO_PKG_VERSION")}),
        )
    } else {
        err_response(&Value::Null, -32001, "auth required")
    };
    if writeln!(write, "{hello}").is_err() || !authed {
        return;
    }

    for line in lines {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Err(_) => err_response(&Value::Null, -32700, "parse error"),
            Ok(req) if !ENABLED.load(Ordering::SeqCst) => {
                err_response(&req.id, -32001, "automation is turned off")
            }
            Ok(req) => {
                let id = req.id.clone();
                let (tx, rx) = mpsc::sync_channel(1);
                QUEUE.lock().unwrap().push_back(Pending { req, reply: tx });
                unsafe { PostMessageW(hwnd as HWND, MSG, 0, 0) };
                match rx.recv_timeout(REPLY_TIMEOUT) {
                    Ok(r) => r,
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                        err_response(&id, -32002, "timeout: the app did not answer")
                    }
                }
            }
        };
        if writeln!(write, "{resp}").is_err() {
            break;
        }
    }
}

// --- UI-thread half ---------------------------------------------------------

/// Serve one parked request. UI thread only.
pub fn respond(app: &mut App, req: &Request) -> String {
    match handle(app, &req.method, &req.params) {
        Ok(v) => ok_response(&req.id, v),
        Err((code, msg)) => err_response(&req.id, code, &msg),
    }
}

type HandleErr = (i64, String);

fn invalid<T>(msg: &str) -> Result<T, HandleErr> {
    Err((-32602, msg.to_owned()))
}

/// `params.layer` (a stable layer id) → index, or the error the client can
/// act on.
fn layer_arg(app: &App, params: &Value) -> Result<usize, HandleErr> {
    let id = params
        .get("layer")
        .and_then(Value::as_u64)
        .ok_or((-32602, "params.layer (a layer id) is required".to_owned()))?;
    app.doc
        .layer_index_of(id)
        .ok_or((-32003, format!("no layer with id {id}")))
}

/// `layer_arg` + the kind check every balloon method opens with, so the
/// error a script gets is the same one from all four.
fn balloon_layer_arg(app: &App, params: &Value) -> Result<usize, HandleErr> {
    let li = layer_arg(app, params)?;
    if app.doc.layers[li].balloons().is_none() {
        return Err((-32602, "not a balloon layer".to_owned()));
    }
    Ok(li)
}

/// Mutations refuse while the app is mid-something: a live text-editor
/// session holds an uncommitted item the whole-set commit would clobber,
/// and a non-empty command queue means we were woken from inside a modal
/// dialog's message pump (see module doc) — jumping the queue would reorder
/// the artist's own actions.
fn busy(app: &App) -> Option<HandleErr> {
    if app.text_edit.is_some() {
        return Some((-32000, "busy: a text edit is open in the app".to_owned()));
    }
    if !app.cmds.is_empty() {
        return Some((-32000, "busy: command queue not idle — retry".to_owned()));
    }
    None
}

fn handle(app: &mut App, method: &str, params: &Value) -> Result<Value, HandleErr> {
    match method {
        "ping" => Ok(json!({"app": "manganakama", "version": env!("CARGO_PKG_VERSION")})),
        "doc.info" => Ok(json!({
            "path": app.doc_path.as_ref().map(|p| p.display().to_string()),
            "page": app.page_index,
            "page_uid": app.pages.get(app.page_index).map(|p| p.uid),
            "pages": app.pages.len(),
            "size": [app.doc.size.0, app.doc.size.1],
            // A plain canvas has NO dpi. `PageSetup::dpi == 0` is core's own
            // "pixel preset, no mm geometry" sentinel (core::page), and a 0
            // on the wire is a number a script would happily divide by —
            // mm→px on a pixel canvas would silently come out zero-sized.
            // Absent means absent: null, and the client decides.
            "dpi": match app.doc_dpi() {
                0 => Value::Null,
                d => json!(d),
            },
        })),
        "layers.list" => {
            let rows: Vec<Value> = app
                .doc
                .layers
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    json!({
                        "id": l.id(),
                        "index": i,
                        "name": l.name,
                        "kind": kind_label(l),
                        "folder": l.folder,
                        "depth": l.depth,
                        "visible": l.visible,
                        "opacity": l.opacity,
                        "active": i == app.doc.active,
                    })
                })
                .collect();
            Ok(json!({"layers": rows}))
        }
        "texts.list" => {
            let li = layer_arg(app, params)?;
            let Some(ts) = app.doc.layers[li].texts() else {
                return invalid("not a text layer");
            };
            let items: Vec<Value> = ts
                .texts
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "text": t.text,
                        "vertical": t.vertical,
                        "align": serde_json::to_value(t.align).unwrap_or(Value::Null),
                        "frame_align": serde_json::to_value(t.frame_align).unwrap_or(Value::Null),
                        "font": t.font,
                        "size_pt": t.size_pt,
                        "pos": t.pos,
                        "size": t.size,
                        "rotation": t.rotation,
                        "auto_size": t.auto_size,
                        "color": t.color,
                        "outline_px": t.outline_px,
                    })
                })
                .collect();
            Ok(json!({"items": items}))
        }
        "balloons.list" => {
            let li = balloon_layer_arg(app, params)?;
            let bs = app.doc.layers[li].balloons().expect("checked");
            let ids: Vec<u64> = bs.balloons.iter().map(|b| b.id).collect();
            let items: Vec<Value> = bs
                .balloons
                .iter()
                .map(|b| {
                    json!({
                        "id": b.id,
                        "shape": serde_json::to_value(&b.shape).unwrap_or(Value::Null),
                        "tails": serde_json::to_value(&b.tails).unwrap_or(Value::Null),
                        // Derived, read-only: body grown by the tails. It is
                        // what a script aims lettering at without having to
                        // re-derive an ellipse's extents itself.
                        "bbox": b.bbox(),
                        "width_scale": b.width_scale,
                        "line_color": b.line_color,
                        "fill_color": b.fill_color,
                        "line_opacity": b.line_opacity,
                        "fill_opacity": b.fill_opacity,
                        "fill_tone": serde_json::to_value(b.fill_tone).unwrap_or(Value::Null),
                    })
                })
                .collect();
            // `ids` stays for the clients written against the first spec.
            // `border_px`/`pressure_width` are the SET's, not any one
            // balloon's — readable here, not patchable (see the doc).
            Ok(json!({
                "ids": ids,
                "items": items,
                "border_px": bs.border_px,
                "pressure_width": bs.pressure_width,
            }))
        }
        "balloons.patch" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            let li = balloon_layer_arg(app, params)?;
            let patches: Vec<BalloonPatch> =
                serde_json::from_value(params.get("items").cloned().unwrap_or_default())
                    .map_err(|e| (-32602, format!("params.items: {e}")))?;
            let n = cmd::balloons_patch(app, li, &patches);
            if n > 0 {
                app.set_status(format!("automation: {n} balloon(s) updated"));
                app.mark_dirty();
            }
            Ok(json!({"patched": n}))
        }
        "balloons.add" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            let li = balloon_layer_arg(app, params)?;
            let adds: Vec<NewBalloon> =
                serde_json::from_value(params.get("items").cloned().unwrap_or_default())
                    .map_err(|e| (-32602, format!("params.items: {e}")))?;
            if adds.is_empty() {
                return invalid("params.items is empty");
            }
            let ink = app.balloon_ink;
            let items: Vec<mn_core::Balloon> = adds.into_iter().map(|a| a.build(ink)).collect();
            // Geometry is checked BEFORE anything commits: a bubble too
            // small to ink is a client bug, not a race, and half a batch is
            // a worse answer than none of it. (The tool refuses the same
            // drag with "draw a closed bubble shape".)
            if let Some(i) = items.iter().position(|b| !b.is_valid()) {
                return invalid(&format!(
                    "items[{i}]: the shape is too small or not closed to be a balloon"
                ));
            }
            let ids = cmd::balloons_add(app, li, items);
            if !ids.is_empty() {
                app.set_status(format!("automation: {} balloon(s) added", ids.len()));
                app.mark_dirty();
            }
            Ok(json!({"ids": ids}))
        }
        "balloons.remove" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            let li = balloon_layer_arg(app, params)?;
            let ids: Vec<u64> =
                serde_json::from_value(params.get("ids").cloned().unwrap_or_default())
                    .map_err(|e| (-32602, format!("params.ids: {e}")))?;
            let n = cmd::balloons_remove(app, li, &ids);
            if n > 0 {
                app.set_status(format!("automation: {n} balloon(s) removed"));
                app.mark_dirty();
            }
            Ok(json!({"removed": n}))
        }
        "layers.add_text" => {
            // Script-driven typesetting from scratch needs somewhere to
            // put the lettering. `Document::add_text_layer` is a commit
            // door: it records its own structure undo and mints the id.
            if let Some(e) = busy(app) {
                return Err(e);
            }
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Lettering");
            let li = app.doc.add_text_layer(name, mn_core::TextSet::default());
            app.set_status(format!("automation: text layer \"{name}\" added"));
            app.mark_dirty();
            Ok(json!({"id": app.doc.layers[li].id(), "index": li}))
        }
        "texts.patch" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            let li = layer_arg(app, params)?;
            let patches: Vec<TextPatch> =
                serde_json::from_value(params.get("items").cloned().unwrap_or_default())
                    .map_err(|e| (-32602, format!("params.items: {e}")))?;
            let n = cmd::texts_patch(app, li, &patches);
            if n > 0 {
                app.set_status(format!("automation: {n} text(s) updated"));
                app.mark_dirty();
            }
            Ok(json!({"patched": n}))
        }
        "texts.add" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            let li = layer_arg(app, params)?;
            let adds: Vec<NewText> =
                serde_json::from_value(params.get("items").cloned().unwrap_or_default())
                    .map_err(|e| (-32602, format!("params.items: {e}")))?;
            if adds.is_empty() {
                return invalid("params.items is empty");
            }
            let template = app.story_item_template(app.page_index);
            let items = adds.into_iter().map(|a| a.build(&template)).collect();
            let ids = cmd::texts_add(app, li, items);
            if !ids.is_empty() {
                app.set_status(format!("automation: {} text(s) added", ids.len()));
                app.mark_dirty();
            }
            Ok(json!({"ids": ids}))
        }
        "texts.remove" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            let li = layer_arg(app, params)?;
            let ids: Vec<u64> =
                serde_json::from_value(params.get("ids").cloned().unwrap_or_default())
                    .map_err(|e| (-32602, format!("params.ids: {e}")))?;
            let n = cmd::texts_remove(app, li, &ids);
            if n > 0 {
                app.set_status(format!("automation: {n} text(s) removed"));
                app.mark_dirty();
            }
            Ok(json!({"removed": n}))
        }
        "pages.list" => {
            let rows: Vec<Value> = app
                .pages
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    json!({
                        "index": i,
                        "uid": p.uid,
                        // The work-folder file identity (`pNNN.ora`), 0 until
                        // a folder save assigns one — the only page number
                        // that survives a restart.
                        "file_id": p.id,
                        "current": i == app.page_index,
                        "spread": p.spread,
                    })
                })
                .collect();
            Ok(json!({"pages": rows}))
        }
        "pages.select" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            // `uid` wins when both are sent: it is the more specific answer
            // to "which page", and a client that learned uids has no reason
            // to also mean the index.
            let page = match params.get("uid").and_then(Value::as_u64) {
                Some(uid) => app
                    .pages
                    .iter()
                    .position(|p| p.uid == uid)
                    .ok_or((-32003, format!("no page with uid {uid}")))?,
                None => {
                    let page = params.get("page").and_then(Value::as_u64).ok_or((
                        -32602,
                        "params.page (an index) or params.uid is required".to_owned(),
                    ))? as usize;
                    if page >= app.pages.len() {
                        return Err((
                            -32003,
                            format!("no page {page} ({} exist)", app.pages.len()),
                        ));
                    }
                    page
                }
            };
            cmd::dispatch(app, AppCmd::SelectPage(page));
            Ok(json!({
                "page": app.page_index,
                "uid": app.pages.get(app.page_index).map(|p| p.uid),
            }))
        }
        "page.render" => {
            let path = params
                .get("path")
                .and_then(Value::as_str)
                .ok_or((-32602, "params.path (a .png path) is required".to_owned()))?;
            if !path.to_ascii_lowercase().ends_with(".png") {
                return invalid("path must end in .png");
            }
            let img = mn_core::export::composite(&app.doc, mn_core::export::Background::White);
            let (w, h) = (img.width(), img.height());
            img.save(path).map_err(|e| (-32004, format!("save failed: {e}")))?;
            Ok(json!({"path": path, "size": [w, h]}))
        }
        "doc.undo" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            cmd::dispatch(app, AppCmd::Undo);
            Ok(json!({}))
        }
        "doc.redo" => {
            if let Some(e) = busy(app) {
                return Err(e);
            }
            cmd::dispatch(app, AppCmd::Redo);
            Ok(json!({}))
        }
        // `auth` after the handshake is a no-op success, so naive clients
        // that re-auth per request still work.
        "auth" => Ok(json!({"ok": true})),
        _ => Err((-32601, format!("unknown method {method}"))),
    }
}

fn kind_label(l: &mn_core::Layer) -> &'static str {
    use mn_core::LayerKind::*;
    match &l.kind {
        Raster => {
            if l.folder {
                "folder"
            } else {
                "raster"
            }
        }
        Fill(_) => "fill",
        Correction(_) => "correction",
        FileObject(_) => "file-object",
        Frame(_) => "frame",
        Balloon(_) => "balloon",
        Text(_) => "text",
    }
}

/// The wire shape for a NEW text item — everything optional but the
/// content; unsupplied fields follow the page's template (the same
/// defaults a story-script field gets).
#[derive(serde::Deserialize)]
struct NewText {
    text: String,
    pos: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    vertical: Option<bool>,
    align: Option<mn_core::Align>,
    frame_align: Option<mn_core::FrameAlign>,
    font: Option<String>,
    size_pt: Option<f32>,
    color: Option<[u8; 3]>,
    outline_px: Option<f32>,
}

impl NewText {
    fn build(self, template: &mn_core::TextItem) -> mn_core::TextItem {
        let mut t = template.clone();
        t.id = 0;
        t.text = self.text;
        t.runs.clear();
        t.cache = None;
        if let Some(p) = self.pos {
            t.pos = p;
        }
        match self.size {
            Some(s) => {
                t.size = s;
                t.auto_size = false;
            }
            None => t.auto_size = true,
        }
        if let Some(v) = self.vertical {
            t.vertical = v;
        }
        if let Some(a) = self.align {
            t.align = a;
        }
        if let Some(a) = self.frame_align {
            t.frame_align = a;
        }
        if let Some(f) = self.font {
            t.font = f;
        }
        if let Some(s) = self.size_pt {
            t.size_pt = s.clamp(1.0, 500.0);
        }
        if let Some(c) = self.color {
            t.color = c;
        }
        if let Some(o) = self.outline_px {
            t.outline_px = o.max(0.0);
        }
        t
    }
}

/// The wire shape for a NEW balloon — everything optional but the shape.
/// Unsupplied fields follow the balloon TOOL's fresh bubble: `Balloon`'s
/// own defaults (no tails, width_scale 1.0) repainted with the Tool
/// Property ink, which is exactly what `canvas_input` builds on a drag.
#[derive(serde::Deserialize)]
struct NewBalloon {
    shape: mn_core::BalloonShape,
    tails: Option<Vec<mn_core::Tail>>,
    width_scale: Option<f32>,
    line_color: Option<[u8; 3]>,
    fill_color: Option<[u8; 3]>,
    line_opacity: Option<f32>,
    fill_opacity: Option<f32>,
    fill_tone: Option<mn_core::BalloonTone>,
}

impl NewBalloon {
    fn build(self, ink: mn_core::BalloonInk) -> mn_core::Balloon {
        let mut b = mn_core::Balloon {
            shape: self.shape,
            tails: self.tails.unwrap_or_default(),
            ..Default::default()
        };
        b.set_ink(ink);
        if let Some(w) = self.width_scale {
            b.width_scale = w.clamp(0.25, 4.0);
        }
        if let Some(c) = self.line_color {
            b.line_color = c;
        }
        if let Some(c) = self.fill_color {
            b.fill_color = c;
        }
        if let Some(o) = self.line_opacity {
            b.line_opacity = o.clamp(0.0, 1.0);
        }
        if let Some(o) = self.fill_opacity {
            b.fill_opacity = o.clamp(0.0, 1.0);
        }
        if let Some(t) = self.fill_tone {
            b.fill_tone = Some(t);
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full socket path without an App: framing, the auth gate, the
    /// queue hand-off and the reply channel. The test thread plays the UI
    /// thread (hwnd 0 = the wake lands in this thread's message queue,
    /// which nobody reads — the poll below is the drain).
    #[test]
    fn socket_auth_gate_and_round_trip() {
        use std::io::{BufRead, BufReader, Write};
        let port = start(0).expect("bind localhost");
        let token = TOKEN.get().unwrap().clone();

        // Wrong token: answered, then dropped.
        let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut w = s;
        writeln!(w, r#"{{"id":1,"method":"auth","params":{{"token":"nope"}}}}"#).unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("auth required"), "{line}");
        line.clear();
        assert_eq!(r.read_line(&mut line).unwrap(), 0, "connection must drop");

        // Right token: hello, then a request that round-trips through the
        // queue.
        let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut w = s;
        writeln!(
            w,
            r#"{{"id":1,"method":"auth","params":{{"token":"{token}"}}}}"#
        )
        .unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("manganakama"), "{line}");

        writeln!(w, r#"{{"id":2,"method":"ping"}}"#).unwrap();
        // Play the UI thread: poll the queue, answer the one request.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let served = loop {
            let mut got = take_pending();
            if let Some(p) = got.pop() {
                break p;
            }
            assert!(std::time::Instant::now() < deadline, "request never queued");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(served.req.method, "ping");
        let _ = served.reply.send(ok_response(&served.req.id, json!({"pong": true})));
        line.clear();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("pong"), "{line}");

        // Gate off: the same connection's next request is refused without
        // reaching the queue.
        stop();
        writeln!(w, r#"{{"id":3,"method":"ping"}}"#).unwrap();
        line.clear();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("turned off"), "{line}");
        assert!(take_pending().is_empty());
        assert!(
            auto_file_path().map(|p| !p.exists()).unwrap_or(true),
            "automation.txt must be gone once the gate is off"
        );
    }

    /// Garbage on an authed line answers a parse error, not a hang or drop.
    #[test]
    fn parse_error_is_answered_inline() {
        // Pure function check — no socket needed.
        assert!(err_response(&Value::Null, -32700, "parse error").contains("-32700"));
        let req: Request = serde_json::from_str(r#"{"method":"ping"}"#).unwrap();
        assert_eq!(req.id, Value::Null);
        assert!(serde_json::from_str::<Request>("not json").is_err());
    }
}
