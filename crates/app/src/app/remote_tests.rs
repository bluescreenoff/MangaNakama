//! Tier 3 automation: the UI-thread half of `remote.rs` against a real
//! headless App — the batch text doors, id addressing, undo grouping and
//! the busy guard. The socket half (framing, auth, queue) tests in
//! `remote.rs` itself without an App.

use crate::cmd::{AppCmd, TextPatch, dispatch};
use crate::remote::{Request, respond};
use serde_json::{Value, json};

fn item(pos: [f32; 2], s: &str) -> mn_core::TextItem {
    let mut t = mn_core::TextItem::new(pos, "Gothic".into(), 9.0, [0, 0, 0], true);
    t.text = s.into();
    let n = t.utf16_len();
    t.runs = vec![mn_core::text::StyleRun::plain(n)];
    t.size = [40.0, 60.0];
    t.auto_size = false;
    t
}

fn call(app: &mut crate::app::App, method: &str, params: Value) -> Value {
    let req = Request {
        id: json!(1),
        method: method.into(),
        params,
    };
    let resp: Value = serde_json::from_str(&respond(app, &req)).expect("valid JSON out");
    resp
}

/// The whole remote text workflow by stable id: list, patch (content +
/// direction + alignment in one batch = ONE undo press), add from the
/// template, remove — and a stale id is skipped, not an error.
#[test]
fn remote_text_batch_by_id_lands_and_undoes_as_one_press() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let li = app.doc.add_text_layer(
        "lettering",
        mn_core::TextSet {
            texts: vec![item([100.0, 100.0], "オイ"), item([300.0, 100.0], "ドン")],
        },
    );
    let lid = app.doc.layers[li].id();
    let ids: Vec<u64> = app.doc.layers[li]
        .texts()
        .unwrap()
        .texts
        .iter()
        .map(|t| t.id)
        .collect();
    assert!(ids.iter().all(|&i| i != 0), "the commit door minted ids");

    // texts.list speaks the same ids.
    let resp = call(&mut app, "texts.list", json!({"layer": lid}));
    let listed: Vec<u64> = resp["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_u64().unwrap())
        .collect();
    assert_eq!(listed, ids);

    // One batch: retext + horizontal on item 0, align on item 1, plus a
    // stale id that must be skipped.
    let resp = call(
        &mut app,
        "texts.patch",
        json!({"layer": lid, "items": [
            {"id": ids[0], "text": "ゴゴゴ", "vertical": false},
            {"id": ids[1], "align": "Center"},
            {"id": 999_999, "text": "nope"},
        ]}),
    );
    assert_eq!(resp["result"]["patched"], 2, "{resp}");
    let ts = app.doc.layers[li].texts().unwrap();
    assert_eq!(ts.texts[0].text, "ゴゴゴ");
    assert!(!ts.texts[0].vertical);
    assert!(
        ts.texts[0].runs.is_empty(),
        "style runs are UTF-16 spans over the OLD string — content change clears them"
    );
    assert_eq!(ts.texts[1].align, mn_core::Align::Center);

    // The batch is one set_texts commit = one Ctrl+Z takes BOTH edits back.
    dispatch(&mut app, AppCmd::Undo);
    let ts = app.doc.layers[li].texts().unwrap();
    assert_eq!(ts.texts[0].text, "オイ", "one undo press reverts the batch");
    assert!(ts.texts[0].vertical);
    assert_eq!(ts.texts[1].align, mn_core::Align::Leading);
    assert_eq!(
        ts.texts.iter().map(|t| t.id).collect::<Vec<_>>(),
        ids,
        "undo restores the same identities"
    );

    // texts.add with only content: template fields (vertical JP) fill in,
    // the door mints a fresh id and reports it.
    let resp = call(
        &mut app,
        "texts.add",
        json!({"layer": lid, "items": [{"text": "ドドド", "pos": [500.0, 200.0]}]}),
    );
    let new_ids: Vec<u64> = resp["result"]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_u64().unwrap())
        .collect();
    assert_eq!(new_ids.len(), 1);
    assert!(!ids.contains(&new_ids[0]), "a fresh mint, not a reuse");
    let ts = app.doc.layers[li].texts().unwrap();
    assert_eq!(ts.texts.len(), 3);
    assert_eq!(ts.texts[2].text, "ドドド");
    assert!(ts.texts[2].vertical, "template default carried");

    // texts.remove by id; the stale id in the list is skipped.
    let resp = call(
        &mut app,
        "texts.remove",
        json!({"layer": lid, "ids": [new_ids[0], 424242]}),
    );
    assert_eq!(resp["result"]["removed"], 1, "{resp}");
    assert_eq!(app.doc.layers[li].texts().unwrap().texts.len(), 2);
}

/// The AppCmd variants drive the same doors (the auto-action surface —
/// `remote.rs` calls the fns directly for their counts).
#[test]
fn texts_batch_appcmds_dispatch_through_the_same_doors() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let li = app.doc.add_text_layer(
        "t",
        mn_core::TextSet {
            texts: vec![item([50.0, 50.0], "a")],
        },
    );
    let id0 = app.doc.layers[li].texts().unwrap().texts[0].id;

    app.push_cmd(AppCmd::TextsPatch {
        layer: li,
        patches: vec![TextPatch {
            id: id0,
            text: Some("b".into()),
            ..Default::default()
        }],
    });
    app.push_cmd(AppCmd::TextsAdd {
        layer: li,
        items: vec![item([80.0, 50.0], "c")],
    });
    while let Some(c) = app.cmds.pop_front() {
        dispatch(&mut app, c);
    }
    let ts = app.doc.layers[li].texts().unwrap();
    assert_eq!(ts.texts[0].text, "b");
    assert_eq!(ts.texts.len(), 2);
    let added = ts.texts[1].id;
    assert!(added != 0 && added != id0);

    app.push_cmd(AppCmd::TextsRemove {
        layer: li,
        ids: vec![added],
    });
    while let Some(c) = app.cmds.pop_front() {
        dispatch(&mut app, c);
    }
    assert_eq!(app.doc.layers[li].texts().unwrap().texts.len(), 1);
}

/// Errors a client can act on: wrong layer id, wrong layer kind, and the
/// busy guard when the command queue is not idle (the modal-dialog wake
/// documented in remote.rs).
#[test]
fn remote_errors_and_busy_guard() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let resp = call(&mut app, "texts.list", json!({"layer": 987654}));
    assert_eq!(resp["error"]["code"], -32003, "{resp}");

    let raster_id = app.doc.layers[0].id();
    let resp = call(&mut app, "texts.list", json!({"layer": raster_id}));
    assert_eq!(resp["error"]["code"], -32602, "not a text layer: {resp}");

    let resp = call(&mut app, "nonsense.method", json!({}));
    assert_eq!(resp["error"]["code"], -32601);

    let li = app.doc.add_text_layer(
        "t",
        mn_core::TextSet {
            texts: vec![item([50.0, 50.0], "a")],
        },
    );
    let lid = app.doc.layers[li].id();
    app.push_cmd(AppCmd::Undo); // queue not idle → mutations must refuse
    let resp = call(
        &mut app,
        "texts.patch",
        json!({"layer": lid, "items": [{"id": 1, "text": "x"}]}),
    );
    assert_eq!(resp["error"]["code"], -32000, "{resp}");
    app.cmds.clear();

    // Queries stay allowed while busy — reading cannot clobber anything.
    app.push_cmd(AppCmd::Undo);
    let resp = call(&mut app, "texts.list", json!({"layer": lid}));
    assert!(resp["result"]["items"].is_array(), "{resp}");
    app.cmds.clear();
}

/// layers.list carries what a script needs to find its target: stable id,
/// kind label, folder flag and the active marker. page.render writes a
/// real PNG of the composite.
#[test]
fn remote_layers_list_and_page_render() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let li = app.doc.add_text_layer(
        "せりふ",
        mn_core::TextSet {
            texts: vec![item([50.0, 50.0], "a")],
        },
    );
    let resp = call(&mut app, "layers.list", json!({}));
    let rows = resp["result"]["layers"].as_array().unwrap().clone();
    assert!(rows.len() >= 2);
    let text_row = rows
        .iter()
        .find(|r| r["name"] == "せりふ")
        .expect("the text layer is listed");
    assert_eq!(text_row["kind"], "text");
    assert_eq!(text_row["id"].as_u64().unwrap(), app.doc.layers[li].id());

    let resp = call(&mut app, "doc.info", json!({}));
    assert_eq!(resp["result"]["pages"], app.pages.len());
    assert!(resp["result"]["size"][0].as_u64().unwrap() > 0);

    // layers.add_text: the from-scratch typesetting door — fresh empty
    // text layer, listed with its minted id, and one undo removes it.
    let n_before = app.doc.layers.len();
    let resp = call(&mut app, "layers.add_text", json!({"name": "写植"}));
    let new_id = resp["result"]["id"].as_u64().unwrap();
    assert!(new_id != 0);
    assert_eq!(app.doc.layers.len(), n_before + 1);
    let resp = call(&mut app, "layers.list", json!({}));
    assert!(
        resp["result"]["layers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "写植" && r["kind"] == "text"),
        "{resp}"
    );
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers.len(), n_before, "structural undo covers it");

    let path = std::env::temp_dir().join("mn-remote-render-test.png");
    let _ = std::fs::remove_file(&path);
    let resp = call(
        &mut app,
        "page.render",
        json!({"path": path.to_string_lossy()}),
    );
    assert!(resp["result"]["path"].is_string(), "{resp}");
    assert!(path.exists(), "the PNG landed");
    let _ = std::fs::remove_file(&path);

    // And the extension gate refuses anything but .png.
    let resp = call(&mut app, "page.render", json!({"path": "C:/nope.exe"}));
    assert_eq!(resp["error"]["code"], -32602);
}
