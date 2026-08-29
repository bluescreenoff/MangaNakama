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

fn bubble(cx: f32, cy: f32) -> mn_core::Balloon {
    mn_core::Balloon {
        shape: mn_core::BalloonShape::Ellipse {
            center: [cx, cy],
            radii: [60.0, 40.0],
        },
        ..Default::default()
    }
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

/// `layers.add_balloon` (tier-3 leftover): the socket could letter INTO
/// balloons but never make one, so a script lettering a page from zero
/// needed a human to click the balloon tool first. The door mirrors
/// `layers.add_text` — one structural undo press — and its border comes
/// from Tool Property, so a scripted bubble is the same weight as a drawn
/// one on the same page.
#[test]
fn remote_layers_add_balloon_lets_a_script_letter_from_zero() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let n_before = app.doc.layers.len();
    let steps = app.doc.undo_labels().len();
    let want_border = app.mm_to_px(app.balloon_border_mm).max(2.0);

    let resp = call(&mut app, "layers.add_balloon", json!({"name": "ふきだし"}));
    let lid = resp["result"]["id"].as_u64().unwrap();
    assert!(lid != 0, "{resp}");
    assert!(
        (resp["result"]["border_px"].as_f64().unwrap() as f32 - want_border).abs() < 1e-3,
        "the border is Tool Property's, not an invented one: {resp}"
    );
    assert_eq!(app.doc.layers.len(), n_before + 1);
    assert_eq!(
        app.doc.undo_labels().len(),
        steps + 1,
        "one structural press, like add_text"
    );
    let li = app.doc.layers.len() - 1;
    assert!(app.doc.layers[li].is_balloon(), "it really is a balloon layer");
    assert_eq!(
        app.doc.layers[li].balloons().unwrap().border_px,
        want_border
    );
    let resp = call(&mut app, "layers.list", json!({}));
    assert!(
        resp["result"]["layers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "ふきだし" && r["kind"] == "balloon"),
        "{resp}"
    );

    // The whole point: the very next call can fill it, no hand click in
    // between. Before this method existed `balloons.add` had no layer to
    // aim at and the run stopped here.
    let resp = call(
        &mut app,
        "balloons.add",
        json!({"layer": lid, "items": [
            {"shape": {"Ellipse": {"center": [300.0, 200.0], "radii": [80.0, 50.0]}}}
        ]}),
    );
    assert_eq!(resp["result"]["ids"].as_array().unwrap().len(), 1, "{resp}");

    // An explicit border overrides Tool Property, floored the same way the
    // tool floors it (a sub-2 px border is a hairline that vanishes in
    // print).
    let resp = call(
        &mut app,
        "layers.add_balloon",
        json!({"name": "thin", "border_px": 0.1}),
    );
    assert!((resp["result"]["border_px"].as_f64().unwrap() - 2.0).abs() < 1e-6, "{resp}");

    // One press per add, all the way back.
    dispatch(&mut app, AppCmd::Undo); // the thin layer
    dispatch(&mut app, AppCmd::Undo); // the balloon add
    dispatch(&mut app, AppCmd::Undo); // the layer itself
    assert_eq!(
        app.doc.layers.len(),
        n_before,
        "structural undo covers the layer"
    );

    // The busy guard is the same one every commit door uses.
    app.push_cmd(AppCmd::Undo);
    let resp = call(&mut app, "layers.add_balloon", json!({}));
    assert_eq!(resp["error"]["code"], -32000, "{resp}");
    app.cmds.clear();
}

/// Balloons get the text items' deal (tier-3 leftover): list carries the
/// geometry, patch/add/remove address items by stable id, one request is
/// one `set_balloons` commit = ONE undo press, a stale id is skipped.
#[test]
fn remote_balloon_batch_by_id_lands_and_undoes_as_one_press() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let li = app.doc.add_balloon_layer(
        "ふきだし",
        mn_core::BalloonSet {
            balloons: vec![bubble(200.0, 200.0), bubble(400.0, 200.0)],
            border_px: 4.0,
            pressure_width: false,
        },
    );
    let lid = app.doc.layers[li].id();
    let ids: Vec<u64> = app.doc.layers[li]
        .balloons()
        .unwrap()
        .balloons
        .iter()
        .map(|b| b.id)
        .collect();
    assert!(ids.iter().all(|&i| i != 0), "the commit door minted ids");

    // balloons.list: the ids it always spoke, plus the geometry a script
    // needs to aim (shape, tails, bbox) and the layer's shared border.
    let resp = call(&mut app, "balloons.list", json!({"layer": lid}));
    let listed: Vec<u64> = resp["result"]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_u64().unwrap())
        .collect();
    assert_eq!(listed, ids);
    let row = &resp["result"]["items"][0];
    assert_eq!(row["id"].as_u64().unwrap(), ids[0]);
    assert_eq!(row["shape"]["Ellipse"]["center"][0], 200.0, "{resp}");
    assert_eq!(row["bbox"], json!([140.0, 160.0, 260.0, 240.0]));
    assert_eq!(resp["result"]["border_px"], 4.0);

    // One batch: reshape + kill the fill on the first, hang a thought tail
    // on the second, and a stale id that must be skipped, not error.
    let resp = call(
        &mut app,
        "balloons.patch",
        json!({"layer": lid, "items": [
            {"id": ids[0],
             "shape": {"Ellipse": {"center": [250.0, 260.0], "radii": [70.0, 50.0]}},
             "fill_opacity": 0.0},
            {"id": ids[1], "tails": [
                {"base": [400.0, 230.0], "tip": [430.0, 320.0], "width": 18.0, "kind": "Thought"}
            ]},
            {"id": 999_999, "width_scale": 2.0},
        ]}),
    );
    assert_eq!(resp["result"]["patched"], 2, "{resp}");
    let bs = app.doc.layers[li].balloons().unwrap();
    assert_eq!(
        bs.balloons[0].shape,
        mn_core::BalloonShape::Ellipse {
            center: [250.0, 260.0],
            radii: [70.0, 50.0]
        }
    );
    assert_eq!(bs.balloons[0].fill_opacity, 0.0);
    assert_eq!(bs.balloons[1].tails.len(), 1);
    assert_eq!(bs.balloons[1].tails[0].kind, mn_core::TailKind::Thought);

    // A patch that would leave a degenerate bubble is skipped the same way
    // an absent id is — the count is the honest answer, and nothing commits.
    let rev = app.doc.revision;
    let resp = call(
        &mut app,
        "balloons.patch",
        json!({"layer": lid, "items": [
            {"id": ids[0], "shape": {"Ellipse": {"center": [250.0, 260.0], "radii": [1.0, 1.0]}}}
        ]}),
    );
    assert_eq!(resp["result"]["patched"], 0, "{resp}");
    assert_eq!(app.doc.revision, rev, "a skipped batch commits nothing");

    // The batch was one commit: one Ctrl+Z takes BOTH edits back.
    dispatch(&mut app, AppCmd::Undo);
    let bs = app.doc.layers[li].balloons().unwrap();
    assert_eq!(
        bs.balloons[0].shape,
        mn_core::BalloonShape::Ellipse {
            center: [200.0, 200.0],
            radii: [60.0, 40.0]
        },
        "one undo press reverts the batch"
    );
    assert_eq!(bs.balloons[0].fill_opacity, 1.0);
    assert!(bs.balloons[1].tails.is_empty());
    assert_eq!(
        bs.balloons.iter().map(|b| b.id).collect::<Vec<_>>(),
        ids,
        "undo restores the same identities"
    );

    // balloons.add with only a shape: the balloon tool's fresh-bubble
    // defaults fill in, the commit door mints and reports the id.
    let resp = call(
        &mut app,
        "balloons.add",
        json!({"layer": lid, "items": [
            {"shape": {"Ellipse": {"center": [600.0, 300.0], "radii": [50.0, 30.0]}}}
        ]}),
    );
    let new_ids: Vec<u64> = resp["result"]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_u64().unwrap())
        .collect();
    assert_eq!(new_ids.len(), 1, "{resp}");
    assert!(!ids.contains(&new_ids[0]), "a fresh mint, not a reuse");
    let bs = app.doc.layers[li].balloons().unwrap();
    assert_eq!(bs.balloons.len(), 3);
    assert_eq!(bs.balloons[2].line_color, [0, 0, 0], "tool ink default");
    assert_eq!(bs.balloons[2].fill_color, [255, 255, 255]);
    assert_eq!(bs.balloons[2].width_scale, 1.0);
    assert!(bs.balloons[2].tails.is_empty());

    // A degenerate ADD is a client bug, not a race: refused outright.
    let resp = call(
        &mut app,
        "balloons.add",
        json!({"layer": lid, "items": [
            {"shape": {"Ellipse": {"center": [600.0, 300.0], "radii": [1.0, 1.0]}}}
        ]}),
    );
    assert_eq!(resp["error"]["code"], -32602, "{resp}");

    // balloons.remove by id; the stale id in the list is skipped.
    let resp = call(
        &mut app,
        "balloons.remove",
        json!({"layer": lid, "ids": [new_ids[0], 424242]}),
    );
    assert_eq!(resp["result"]["removed"], 1, "{resp}");
    assert_eq!(app.doc.layers[li].balloons().unwrap().balloons.len(), 2);

    // Wrong layer kind still errors the way texts.list does.
    let raster_id = app.doc.layers[0].id();
    let resp = call(&mut app, "balloons.patch", json!({"layer": raster_id, "items": []}));
    assert_eq!(resp["error"]["code"], -32602, "{resp}");
}

/// Pages keep their index on the wire (back-compat) but now also carry the
/// runtime `uid`, so a script that reorders pages — or races the artist
/// doing it — can re-find the page it was working on.
#[test]
fn remote_pages_are_findable_by_uid_across_a_reorder() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    super::new_document_tests::small_draft(&mut app, 3, "");
    dispatch(&mut app, AppCmd::NewComicCreate);
    assert_eq!(app.pages.len(), 3, "a three-page draft");

    let resp = call(&mut app, "pages.list", json!({}));
    let rows = resp["result"]["pages"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 3, "{resp}");
    let uids: Vec<u64> = rows.iter().map(|r| r["uid"].as_u64().unwrap()).collect();
    assert!(uids.iter().all(|&u| u != 0), "every page has a uid");
    let mut sorted = uids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 3, "uids are distinct");
    assert_eq!(rows[0]["index"], 0);
    assert_eq!(
        rows.iter().filter(|r| r["current"] == true).count(),
        1,
        "exactly one current page"
    );

    // doc.info speaks the same uid for the page it reports the index of.
    let resp = call(&mut app, "doc.info", json!({}));
    let cur = resp["result"]["page"].as_u64().unwrap() as usize;
    assert_eq!(resp["result"]["page_uid"].as_u64().unwrap(), uids[cur]);

    // Index addressing still works, and answers with the uid.
    let resp = call(&mut app, "pages.select", json!({"page": 2}));
    assert_eq!(resp["result"]["page"], 2, "{resp}");
    assert_eq!(resp["result"]["uid"].as_u64().unwrap(), uids[2]);

    // The artist reorders. The index the script remembered now names a
    // DIFFERENT page; the uid still names its own.
    dispatch(&mut app, AppCmd::MovePage { from: 2, to: 0 });
    let resp = call(&mut app, "pages.select", json!({"uid": uids[2]}));
    assert_eq!(resp["result"]["page"], 0, "{resp}");
    assert_eq!(resp["result"]["uid"].as_u64().unwrap(), uids[2]);

    let resp = call(&mut app, "pages.select", json!({"uid": 777_777}));
    assert_eq!(resp["error"]["code"], -32003, "{resp}");
}

/// A plain canvas has no dpi — `PageSetup::dpi == 0` is core's own "no mm
/// geometry" sentinel, and 0 on the wire is a number a script would divide
/// by. Absent means absent (tier-3 leftover: doc.info reported 0).
#[test]
fn doc_info_reports_no_dpi_on_a_plain_canvas() {
    let Some(mut app) = super::new_document_tests::headless() else {
        return;
    };
    let resp = call(&mut app, "doc.info", json!({}));
    assert!(
        resp["result"]["dpi"].is_null(),
        "a pixel canvas has no dpi: {resp}"
    );

    // A comic page does have one, and reports the number.
    super::new_document_tests::small_draft(&mut app, 1, "");
    dispatch(&mut app, AppCmd::NewComicCreate);
    let resp = call(&mut app, "doc.info", json!({}));
    assert_eq!(resp["result"]["dpi"], 72, "{resp}");
}
