# Automation — the remote socket and the MCP shim

MangaNakama can be driven by scripts and AI assistants while it runs: a
localhost JSON-RPC socket injects work into the same command queue the UI
uses, so every remote edit is undoable (Ctrl+Z), visible live, and subject
to the same rules as a click. Born from JP typesetting: batch per-item
align / direction / content edits that no auto-action could reach.

## Turning it on

Preferences ▸ Performance ▸ **Automation server** (off by default; the
setting is `automation=` in `prefs.txt`, and gibberish there reads as OFF).
While on, the app listens on an ephemeral `127.0.0.1` port and writes

    automation.txt        (beside the exe)
    port=<u16>
    token=<32 hex chars, new every launch>

Turning the preference off gates the socket immediately (auth and requests
refuse; the port unbinds at exit) and deletes `automation.txt`. A stale
file from a crash is deleted at the next launch with the setting off.

## Why this shape (trust boundary)

The socket is an arbitrary-command port into the open document, so:

- **localhost bind only**, never configurable outward.
- **Raw newline-delimited JSON over TCP, not HTTP** — a web page cannot
  speak it (`fetch` has no raw sockets), which closes the
  browser-on-the-same-machine hole an HTTP port would open.
- **Per-launch token**: the first line of every connection must be
  `auth` with the token from `automation.txt`, or the connection is
  answered `auth required` and dropped.
- **Single-writer discipline**: connection threads only park requests and
  wake the UI thread (`remote.rs` module doc); all Document access happens
  on the UI thread between frames. Mutations answer `busy` (-32000) while
  a text edit is open in the app or the command queue is mid-dialog —
  retry after a moment.

## Protocol

One JSON-RPC 2.0 request per line, one response per line:

    {"id":1,"method":"auth","params":{"token":"…"}}
    {"id":1,"result":{"app":"manganakama","version":"0.2.0"}}

Methods (`layer` params take the **stable layer id** from `layers.list`;
text items are addressed by their stable ids from `texts.list`):

| method          | params                          | result |
|-----------------|---------------------------------|--------|
| `ping`          | —                               | app + version |
| `doc.info`      | —                               | path, page, pages, size, dpi |
| `layers.list`   | —                               | id, index, name, kind, folder, depth, visible, opacity, active |
| `texts.list`    | `{layer}`                       | items: id, text, vertical, align, frame_align, font, size_pt, pos, size, rotation, auto_size, color, outline_px |
| `balloons.list` | `{layer}`                       | ids |
| `layers.add_text` | `{name?}`                     | `{id, index}` — a fresh empty text layer, undoable |
| `texts.patch`   | `{layer, items:[{id, …fields}]}`| `{patched}` — absent ids are skipped |
| `texts.add`     | `{layer, items:[{text, …}]}`    | `{ids}` minted by the commit door |
| `texts.remove`  | `{layer, ids}`                  | `{removed}` |
| `pages.select`  | `{page}` (index)                | `{page}` |
| `page.render`   | `{path}` (must end `.png`)      | `{path, size}` |
| `doc.undo` / `doc.redo` | —                       | `{}` |

Semantics worth knowing:

- One `texts.patch`/`add`/`remove` request = ONE undo press, however many
  items it touched (`Document::set_texts` whole-set commit).
- Patching `text` clears the item's style runs (spans are UTF-16 offsets
  into the old string). Patching `size` turns `auto_size` off, same as a
  hand resize. `align` is `Leading|Center|Trailing`, `frame_align` is
  `Near|Center|Far`, `vertical:true` is JP columns.
- `texts.add` fills unspecified fields from the page's template — the
  same defaults a story-script field gets (typically vertical JP
  lettering styled like the page's last text item).
- Errors: `-32700` parse, `-32601` unknown method, `-32602` bad params,
  `-32000` busy (retry), `-32001` auth/off, `-32002` timeout, `-32003`
  unknown layer/page, `-32004` render failed.

Claude Code (or any script) can speak this directly — open a TCP
connection, auth, send lines.

## MCP

`mn-mcp.exe` (crates/mcp) is a deliberately separate shim binary: stdio
MCP on one side, the socket on the other, one tool per socket method.
The app embeds no MCP machinery — if the protocol moves, the shim moves.

    claude mcp add manganakama -- <install>\mn-mcp.exe

The shim reads `automation.txt` beside its own exe (ship it next to
`manganakama.exe`), or pass `--auto-file <path>`. It reconnects once per
call on failure, so restarting the app mid-session just works.

## Testing

- Socket half (framing, auth gate, queue hand-off, live off-switch):
  `remote.rs` tests, no App needed.
- UI-thread half (batch doors by id, one-press undo, busy guard, errors,
  page render): `app/remote_tests.rs` against a headless App.
- Shim surface: `crates/mcp` unit tests.
