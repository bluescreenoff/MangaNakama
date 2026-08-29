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
text items and balloons are addressed by their stable ids from
`texts.list` / `balloons.list`):

| method          | params                          | result |
|-----------------|---------------------------------|--------|
| `ping`          | —                               | app + version |
| `doc.info`      | —                               | path, page, page_uid, pages, size, dpi |
| `layers.list`   | —                               | id, index, name, kind, folder, depth, visible, opacity, active |
| `texts.list`    | `{layer}`                       | items: id, text, vertical, align, frame_align, font, size_pt, pos, size, rotation, auto_size, color, outline_px |
| `balloons.list` | `{layer}`                       | ids; items: id, shape, tails, bbox, width_scale, line_color, fill_color, line_opacity, fill_opacity, fill_tone; border_px, pressure_width |
| `layers.add_text` | `{name?}`                     | `{id, index}` — a fresh empty text layer, undoable |
| `texts.patch`   | `{layer, items:[{id, …fields}]}`| `{patched}` — absent ids are skipped |
| `texts.add`     | `{layer, items:[{text, …}]}`    | `{ids}` minted by the commit door |
| `texts.remove`  | `{layer, ids}`                  | `{removed}` |
| `balloons.patch`| `{layer, items:[{id, …fields}]}`| `{patched}` — absent ids skipped |
| `balloons.add`  | `{layer, items:[{shape, …}]}`   | `{ids}` minted by the commit door |
| `balloons.remove` | `{layer, ids}`                | `{removed}` |
| `pages.list`    | —                               | pages: index, uid, file_id, current, spread |
| `pages.select`  | `{uid}` or `{page}` (index)     | `{page, uid}` |
| `page.render`   | `{path}` (must end `.png`)      | `{path, size}` |
| `doc.undo` / `doc.redo` | —                       | `{}` |

Semantics worth knowing:

- One `texts.patch`/`add`/`remove` request = ONE undo press, however many
  items it touched (`Document::set_texts` whole-set commit). The three
  `balloons.*` doors are the same deal through `set_balloons`.
- Patching `text` clears the item's style runs (spans are UTF-16 offsets
  into the old string). Patching `size` turns `auto_size` off, same as a
  hand resize. `align` is `Leading|Center|Trailing`, `frame_align` is
  `Near|Center|Far`, `vertical:true` is JP columns.
- `texts.add` fills unspecified fields from the page's template — the
  same defaults a story-script field gets (typically vertical JP
  lettering styled like the page's last text item).

### Balloons

A balloon's `shape` carries its position, size AND kind together, so a
move sends the whole shape back. It is Rust's externally-tagged JSON —
exactly what `balloons.list` prints:

    {"Ellipse":   {"center":[x,y], "radii":[rx,ry]}}
    {"RoundRect": {"rect":[x0,y0,x1,y1], "corner":px}}
    {"Polygon":   {"points":[[x,y],…], "widths":[…], "corners":[…]}}

`tails` is the whole list, replaced (tails have no ids of their own):
`{"base":[x,y], "tip":[x,y], "width":px, "kind":"Spoken|Thought|Spike",
"bend":0.0}`. Style is `width_scale` (0.25–4, CSP's "correct line width"),
`line_color`/`fill_color` `[r,g,b]`, `line_opacity`/`fill_opacity` 0–1
(**fill 0 = CSP's "fill inside frame" off**: the art shows through), and
`fill_tone` `{cell_px, angle_deg, density, pattern}` for a screened bubble
— send `"fill_tone": null` to go back to a flat fill.

`balloons.add` fills the rest from the balloon tool's fresh bubble (no
tails, `width_scale` 1.0, Tool Property's current ink). A shape too small
or not closed to be a balloon is refused on `add` (`-32602`, nothing
commits) and skipped on `patch` (it does not count toward `patched`) —
the same refusal the tool gives a stray drag.

Read-only in the reply, and deliberately not patchable: `bbox` (the body
grown by its tails — aim lettering with it) is derived, and `border_px` /
`pressure_width` belong to the LAYER's balloon set, not to any one bubble.

### Pages

Pages stay index-addressed for back-compat, but every page also carries a
`uid`: stable for the session, whatever index the page drifts to. Read it
from `pages.list` (or `doc.info.page_uid` for the current one), then send
`pages.select {"uid": …}` to land on that page again after a reorder — by
you or by the artist. `uid` wins if both params are sent; an unknown one
is `-32003`. It is a RUNTIME identity: not persisted, gone at restart.
`file_id` is the persisted one (`pNNN.ora` in a work folder), and is `0`
until the work has been saved to a folder.

### dpi

`doc.info.dpi` is **null on a plain pixel canvas** — a canvas with no page
setup has no physical size, and reporting `0` invited a client to compute
`mm / 25.4 * dpi` and silently get nothing. A comic page reports its real
number. (`PageSetup::dpi == 0` is the same sentinel inside the app; the
socket just refuses to spell it as a number.)
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
