//! Surface pass — lettering end to end, the way a mangaka does it: click,
//! type Japanese down a column, draw a bubble round it, hang a tail on
//! it, move the bubble and watch the words come along. Every flow drives
//! the real doors (`canvas_down/move/up`, `text_char`, `text_key`,
//! `AppCmd`s) and renders the page, so a run with `MN_SURFACE_OUT=<dir>`
//! leaves one PNG per flow to look at. Without it the assertions alone
//! stand: things land where they should, and the page inks where the
//! lettering is and nowhere else.

use crate::app::{App, PointerKind, headless_renderer};
use crate::cmd::{AppCmd, BalloonMode, Tool, dispatch};
use mn_core::text::StyleFlag;
use mn_core::{Balloon, BalloonShape, PenSample, Tail, TailKind, TextItem};

const NONE: [PenSample; 0] = [];

/// A 900×700 page at the identity viewport, so canvas == client px.
fn app() -> Option<App> {
    let renderer = headless_renderer()?;
    let mut app = App::new(renderer, (900, 700), 1.0);
    app.doc = mn_core::Document::new(900, 700);
    app.viewport = mn_gpu::Viewport::default();
    app.text_engine.as_ref()?;
    Some(app)
}

fn pump(app: &mut App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

fn type_str(app: &mut App, s: &str) {
    // A newline is the Enter KEY (VK_RETURN), not a character: `text_char`
    // drops control units the way WM_CHAR does.
    for c in s.chars() {
        if c == '\n' {
            app.text_key(0x0D, false, false);
            continue;
        }
        let mut buf = [0u16; 2];
        for u in c.encode_utf16(&mut buf) {
            app.text_char(*u);
        }
    }
}

fn select(app: &mut App, a: u32, b: u32) {
    if let Some(ed) = app.text_edit.as_mut() {
        ed.anchor = a;
        ed.caret = b;
    }
}

fn enter(app: &mut App) {
    app.text_key(0x0D, false, false);
}

fn esc(app: &mut App) {
    app.text_key(0x1B, false, false);
    pump(app);
}

fn click(app: &mut App, p: (f32, f32)) {
    app.canvas_down(p.0, p.1, PointerKind::Pen, &NONE);
    app.canvas_up(p.0, p.1, &NONE);
    pump(app);
}

fn drag(app: &mut App, a: (f32, f32), b: (f32, f32)) {
    app.canvas_down(a.0, a.1, PointerKind::Pen, &NONE);
    let steps = 8;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        app.canvas_move(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, &NONE);
    }
    app.canvas_up(b.0, b.1, &NONE);
    pump(app);
}

/// Render the page 1:1 and, when asked, keep it.
fn render(app: &mut App, name: &str) -> image::RgbaImage {
    let (w, h) = app.doc.size;
    let img = app.renderer.render_offscreen(&app.doc, w, h);
    if let Ok(dir) = std::env::var("MN_SURFACE_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        img.save(format!("{dir}/{name}.png")).expect("png written");
    }
    img
}

/// Bounding box of the non-white ink, `[x0, y0, x1, y1]` exclusive.
fn ink_bbox(img: &image::RgbaImage) -> Option<[u32; 4]> {
    let mut bb: Option<[u32; 4]> = None;
    for (x, y, p) in img.enumerate_pixels() {
        let dark = (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 200 && p[3] > 0;
        if dark {
            bb = Some(match bb {
                None => [x, y, x + 1, y + 1],
                Some(b) => [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)],
            });
        }
    }
    bb
}

fn ink_in(img: &image::RgbaImage, r: [u32; 4]) -> usize {
    let mut n = 0;
    for y in r[1]..r[3].min(img.height()) {
        for x in r[0]..r[2].min(img.width()) {
            let p = img.get_pixel(x, y);
            if (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 200 && p[3] > 0 {
                n += 1;
            }
        }
    }
    n
}

fn text_layers(app: &App) -> Vec<usize> {
    (0..app.doc.layers.len())
        .filter(|&i| app.doc.layers[i].texts().is_some())
        .collect()
}

fn item(app: &App, li: usize, ti: usize) -> &TextItem {
    &app.doc.layers[li].texts().unwrap().texts[ti]
}

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Balloon {
    Balloon {
        shape: BalloonShape::Ellipse {
            center: [cx, cy],
            radii: [rx, ry],
        },
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------

/// CSP "Adding text": click, type, done. Vertical Japanese with the
/// punctuation a real balloon has — 、。「」ー… and the ！ at the end.
#[test]
fn click_type_vertical_japanese_lands_one_item_and_inks_a_column() {
    let Some(mut app) = app() else { return };
    app.tool = Tool::Text;
    app.text_size_pt = 18.0;
    click(&mut app, (600.0, 120.0));
    assert!(app.text_edit.is_some(), "a click opens a box");
    type_str(&mut app, "こんにちは、世界。\n「ふきだし」です…ー！");
    esc(&mut app);
    assert!(app.text_edit.is_none(), "Esc closes it");
    let tl = text_layers(&app);
    assert_eq!(tl.len(), 1, "one text layer");
    let it = item(&app, tl[0], 0);
    assert!(it.vertical, "new text is vertical by default (JP work)");
    assert_eq!(it.text, "こんにちは、世界。\n「ふきだし」です…ー！");
    let img = render(&mut app, "t01_vertical_jp");
    let bb = ink_bbox(&img).expect("the column inked");
    assert!(
        bb[3] - bb[1] > bb[2] - bb[0],
        "a vertical column is taller than wide: {bb:?}"
    );
    assert!(
        bb[2] as f32 <= 600.0 + 4.0,
        "the column hangs LEFT of the click (top-right corner planted): {bb:?}"
    );
}

/// CSP "drag to make a text box, wrap at frame". Horizontal English in a
/// dragged box must wrap inside it, not run off the page.
#[test]
fn drag_box_horizontal_english_wraps_inside_the_box() {
    let Some(mut app) = app() else { return };
    app.tool = Tool::Text;
    app.text_vertical = false;
    app.text_size_pt = 14.0;
    drag(&mut app, (100.0, 100.0), (400.0, 260.0));
    assert!(app.text_edit.is_some(), "a drag opens a fixed box");
    type_str(
        &mut app,
        "The quick brown fox jumps over the lazy dog, and keeps running until the box says stop.",
    );
    esc(&mut app);
    let tl = text_layers(&app);
    let it = item(&app, tl[0], 0);
    assert!(!it.auto_size, "a dragged box is fixed, not auto");
    let img = render(&mut app, "t02_horizontal_en_box");
    let bb = ink_bbox(&img).expect("inked");
    assert!(bb[2] <= 404, "wrapped inside the 300 px box: {bb:?}");
    assert!(bb[3] - bb[1] > 30, "more than one line: {bb:?}");
}

/// CSP "Editing text": Object tool selects, drag moves, corner resizes,
/// the lollipop rotates, Del deletes, Ctrl+Z brings it back.
#[test]
fn object_tool_moves_resizes_rotates_and_deletes_a_text() {
    let Some(mut app) = app() else { return };
    app.tool = Tool::Text;
    app.text_vertical = false;
    app.text_size_pt = 16.0;
    click(&mut app, (200.0, 200.0));
    type_str(&mut app, "MOVE ME");
    esc(&mut app);
    let li = text_layers(&app)[0];
    let before = item(&app, li, 0).clone();
    assert!(before.size[0] > 20.0 && before.size[1] > 10.0, "{:?}", before.size);
    let c = before.center();

    app.tool = Tool::Object;
    // Move by (+150, +100).
    drag(&mut app, (c[0], c[1]), (c[0] + 150.0, c[1] + 100.0));
    let moved = item(&app, li, 0).clone();
    assert!(
        (moved.pos[0] - before.pos[0] - 150.0).abs() < 1.0
            && (moved.pos[1] - before.pos[1] - 100.0).abs() < 1.0,
        "moved by the drag: {:?} -> {:?}",
        before.pos,
        moved.pos
    );
    assert_eq!(app.text_sel, Some((li, 0)), "and stays selected");
    render(&mut app, "t03a_text_moved");

    // Resize: grab corner 2 (x1,y1) and pull it out.
    let rot = crate::app::ROTATE_STALK_SCREEN;
    let corner = moved
        .handles(rot)
        .into_iter()
        .find(|(_, h)| *h == mn_core::text::TextHandle::Corner(2))
        .unwrap()
        .0;
    drag(&mut app, (corner[0], corner[1]), (corner[0] + 80.0, corner[1] + 40.0));
    let resized = item(&app, li, 0).clone();
    assert!(
        resized.size[0] > moved.size[0] + 60.0,
        "the box grew: {:?} -> {:?}",
        moved.size,
        resized.size
    );
    assert!(!resized.auto_size, "a hand-resized box is fixed");
    assert!(
        (resized.size_pt - moved.size_pt).abs() < 1e-3,
        "resizing the box does not scale the type"
    );

    // Rotate: the lollipop above the top edge, dragged a quarter turn.
    let lolly = resized
        .handles(rot)
        .into_iter()
        .find(|(_, h)| *h == mn_core::text::TextHandle::Rotate)
        .unwrap()
        .0;
    let c = resized.center();
    drag(&mut app, (lolly[0], lolly[1]), (c[0] + (c[1] - lolly[1]), c[1]));
    let turned = item(&app, li, 0).clone();
    assert!(
        (turned.rotation - std::f32::consts::FRAC_PI_2).abs() < 0.05,
        "a quarter turn: {}",
        turned.rotation
    );
    let img = render(&mut app, "t03b_text_rotated");
    let bb = ink_bbox(&img).unwrap();
    assert!(bb[3] - bb[1] > bb[2] - bb[0], "rotated text is taller than wide");

    // Delete + undo.
    dispatch(&mut app, AppCmd::TextDelete { layer: li, text: 0 });
    assert!(app.doc.layers[li].texts().unwrap().texts.is_empty());
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(app.doc.layers[li].texts().unwrap().texts.len(), 1, "undo restores it");
}

/// CSP: double-click a text with the Object tool to edit it in place.
#[test]
fn double_click_edits_in_place_and_keeps_typing() {
    let Some(mut app) = app() else { return };
    app.tool = Tool::Text;
    app.text_vertical = false;
    click(&mut app, (300.0, 300.0));
    type_str(&mut app, "Hello");
    esc(&mut app);
    let li = text_layers(&app)[0];
    let c = item(&app, li, 0).center();
    app.tool = Tool::Object;
    // Two presses inside 400 ms at the same spot = a double-click.
    app.canvas_down(c[0], c[1], PointerKind::Pen, &NONE);
    app.canvas_up(c[0], c[1], &NONE);
    app.canvas_down(c[0], c[1], PointerKind::Pen, &NONE);
    app.canvas_up(c[0], c[1], &NONE);
    pump(&mut app);
    assert!(app.text_edit.is_some(), "the double-click opened the box");
    assert_eq!(app.tool, Tool::Text, "and handed over to the Text tool");
    app.text_key(0x23, true, false); // Ctrl+End
    type_str(&mut app, ", world");
    esc(&mut app);
    assert_eq!(item(&app, li, 0).text, "Hello, world");
    render(&mut app, "t04_edit_in_place");
}

/// CSP Font / Line space / Alignment rows: every knob reaches the
/// selected item and re-shapes it.
#[test]
fn font_size_leading_tracking_and_alignment_reshape_the_item() {
    let Some(mut app) = app() else { return };
    app.tool = Tool::Text;
    app.text_size_pt = 14.0;
    click(&mut app, (700.0, 80.0));
    type_str(&mut app, "明朝体で\n組んだ台詞\nのテスト");
    esc(&mut app);
    let li = text_layers(&app)[0];
    app.text_sel = Some((li, 0));
    let base = item(&app, li, 0).clone();
    let img0 = render(&mut app, "t05a_base");
    let bb0 = ink_bbox(&img0).unwrap();

    // Size up.
    app.apply_text_prop(|t| t.size_pt = 24.0);
    pump(&mut app);
    let img1 = render(&mut app, "t05b_size24");
    let bb1 = ink_bbox(&img1).unwrap();
    assert!(bb1[3] - bb1[1] > bb0[3] - bb0[1], "bigger type, taller column");

    // Leading: 200 % spreads the columns apart (vertical: line = column).
    app.apply_text_prop(|t| t.line_spacing = mn_core::text::LineSpacing::Percent(200.0));
    pump(&mut app);
    let img2 = render(&mut app, "t05c_leading200");
    let bb2 = ink_bbox(&img2).unwrap();
    assert!(bb2[2] - bb2[0] > bb1[2] - bb1[0], "wider column spread");

    // Tracking.
    app.apply_text_prop(|t| t.letter_spacing_pt = 4.0);
    pump(&mut app);
    let img3 = render(&mut app, "t05d_tracking4");
    let bb3 = ink_bbox(&img3).unwrap();
    assert!(bb3[3] - bb3[1] > bb2[3] - bb2[1], "tracking lengthens the column");

    // Font: a Mincho if the machine has one, else any second family.
    let fam = {
        let e = app.text_engine.as_ref().unwrap();
        e.families()
            .iter()
            .find(|f| f.contains("明朝") || f.contains("Mincho"))
            .or_else(|| e.families().iter().find(|f| **f != base.font))
            .cloned()
    };
    if let Some(fam) = fam {
        app.apply_text_prop(|t| t.font = fam.clone());
        pump(&mut app);
        assert_eq!(item(&app, li, 0).font, fam);
        render(&mut app, "t05e_mincho");
    }

    // Alignment: Center then Trailing, both must move the ink.
    app.apply_text_prop(|t| {
        t.auto_size = false;
        t.size = [220.0, 300.0];
        t.align = mn_core::Align::Center;
    });
    pump(&mut app);
    let img4 = render(&mut app, "t05f_align_center");
    app.apply_text_prop(|t| t.align = mn_core::Align::Trailing);
    pump(&mut app);
    let img5 = render(&mut app, "t05g_align_trailing");
    assert_ne!(ink_bbox(&img4), ink_bbox(&img5), "alignment moved the block");
    let n = app.doc.undo_labels().len();
    assert!(n >= 5, "every knob is its own undo step: {n}");
}

/// Mixed JP + EN in one balloon: the English gets its own face, then
/// bold. 縦中横 stands the digits up; a reading rides the kanji.
#[test]
fn mixed_jp_en_range_font_bold_tcy_and_ruby() {
    let Some(mut app) = app() else { return };
    app.tool = Tool::Text;
    app.text_size_pt = 20.0;
    click(&mut app, (500.0, 60.0));
    type_str(&mut app, "第1話：22時にBOSSが来る");
    // "BOSS" = units 8..12
    select(&mut app, 8, 12);
    let latin = app
        .text_engine
        .as_ref()
        .unwrap()
        .families()
        .iter()
        .find(|f| *f == "Arial" || *f == "Impact")
        .cloned()
        .unwrap_or_else(|| app.text_engine.as_ref().unwrap().default_family());
    app.text_font_range_button(latin.clone());
    app.text_style_button(StyleFlag::Bold);
    // 縦中横 on the "22".
    select(&mut app, 5, 7);
    app.text_tcy_button();
    // Reading on 第1話 -> "だいいちわ" (select 0..3).
    select(&mut app, 0, 3);
    app.text_ruby = "だいいちわ".into();
    app.text_ruby_button();
    esc(&mut app);
    let li = text_layers(&app)[0];
    let it = item(&app, li, 0);
    assert_eq!(it.fonts.len(), 1, "one font override run");
    assert_eq!(it.fonts[0].family, latin);
    assert!(it.range_has_all(8, 12, StyleFlag::Bold));
    assert!(!it.tcy.is_empty() || !it.effective_tcy().is_empty(), "22 stands up");
    assert_eq!(it.ruby.len(), 1, "one reading");
    assert_eq!(it.ruby[0].text, "だいいちわ");
    let img = render(&mut app, "t06_mixed_tcy_ruby");
    assert!(ink_bbox(&img).is_some());
}

/// CSP Balloon tool group: Ellipse balloon, Rounded rectangle, the drawn
/// (Balloon pen) bubble. Each is one drag; each lands selected.
#[test]
fn balloon_shapes_ellipse_round_and_drawn() {
    let Some(mut app) = app() else { return };
    app.tool = Tool::Balloon;
    app.balloon_mode = BalloonMode::Ellipse;
    drag(&mut app, (80.0, 80.0), (320.0, 260.0));
    assert!(app.balloon_sel.is_some(), "the fresh balloon is selected");
    app.balloon_mode = BalloonMode::Round;
    drag(&mut app, (380.0, 80.0), (620.0, 260.0));
    app.balloon_mode = BalloonMode::Draw;
    // A wobbly freehand loop around (250, 480), r≈120, pressure varying.
    let n = 48;
    let pt = |i: usize| {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let r = 120.0 + 18.0 * (a * 5.0).sin();
        (250.0 + r * a.cos(), 480.0 + r * a.sin())
    };
    let sample = |i: usize| {
        let (x, y) = pt(i);
        [PenSample {
            x,
            y,
            pressure: 0.35 + 0.5 * (0.5 + 0.5 * (i as f32 / n as f32 * std::f32::consts::TAU).sin()),
            tilt_x: 0.0,
            tilt_y: 0.0,
            t_ms: i as f64 * 8.0,
        }]
    };
    let (x0, y0) = pt(0);
    app.canvas_down(x0, y0, PointerKind::Pen, &sample(0));
    for i in 1..n {
        let (x, y) = pt(i);
        app.canvas_move(x, y, &sample(i));
    }
    app.canvas_up(x0, y0, &sample(0));
    pump(&mut app);

    let bl: Vec<usize> = (0..app.doc.layers.len())
        .filter(|&i| app.doc.layers[i].is_balloon())
        .collect();
    let total: usize = bl.iter().map(|&i| app.doc.layers[i].balloons().unwrap().balloons.len()).sum();
    assert_eq!(total, 3, "three bubbles");
    let img = render(&mut app, "t07_balloon_shapes");
    // Outline ink: the ellipse's leftmost edge, the round rect's top edge.
    assert!(ink_in(&img, [78, 160, 86, 180]) > 0, "ellipse left edge inked");
    assert!(ink_in(&img, [480, 78, 520, 86]) > 0, "round rect top edge inked");
    assert!(ink_in(&img, [140, 140, 260, 200]) == 0, "ellipse interior is paper");
}

/// CSP "Adding a balloon tail": press inside, drag out. Three kinds and a
/// bend, each a visible join on the body.
#[test]
fn balloon_tails_spoken_thought_spike_and_bent() {
    let Some(mut app) = app() else { return };
    let mut bs = mn_core::BalloonSet::new(4.0);
    bs.balloons.push(ellipse(160.0, 160.0, 110.0, 80.0));
    bs.balloons.push(ellipse(460.0, 160.0, 110.0, 80.0));
    bs.balloons.push(ellipse(760.0, 160.0, 110.0, 80.0));
    bs.balloons.push(ellipse(300.0, 480.0, 110.0, 80.0));
    let li = app.doc.add_balloon_layer("bubbles", bs);
    app.doc.active = li;
    app.tool = Tool::Balloon;
    app.balloon_mode = BalloonMode::Tail;

    app.balloon_tail_kind = TailKind::Spoken;
    drag(&mut app, (160.0, 200.0), (120.0, 330.0));
    app.balloon_tail_kind = TailKind::Thought;
    drag(&mut app, (460.0, 200.0), (420.0, 330.0));
    app.balloon_tail_kind = TailKind::Spike;
    drag(&mut app, (760.0, 200.0), (720.0, 330.0));
    app.balloon_tail_kind = TailKind::Spoken;
    app.balloon_tail_bend = 0.45;
    drag(&mut app, (300.0, 520.0), (520.0, 640.0));
    let bs = app.doc.layers[li].balloons().unwrap();
    for (i, k) in [TailKind::Spoken, TailKind::Thought, TailKind::Spike, TailKind::Spoken]
        .iter()
        .enumerate()
    {
        assert_eq!(bs.balloons[i].tails.len(), 1, "balloon {i} got its tail");
        assert_eq!(bs.balloons[i].tails[0].kind, *k);
    }
    assert!((bs.balloons[3].tails[0].bend - 0.45).abs() < 1e-6);
    let img = render(&mut app, "t08_balloon_tails");
    // The spoken tail's tip region inks; a bend puts ink off the chord.
    assert!(ink_in(&img, [110, 300, 140, 335]) > 0, "spoken tip inked");
    // A tail drag that starts OUTSIDE a balloon is refused with a message.
    drag(&mut app, (600.0, 600.0), (700.0, 650.0));
    assert!(app.status.contains("inside a balloon"), "{}", app.status);
}

/// CSP "Moving balloons / Transforming balloons": the lettering inside a
/// bubble comes along when the bubble moves, and stays at its fraction
/// when the bubble is resized — the type size untouched.
#[test]
fn moving_and_resizing_a_balloon_carries_the_lettering() {
    let Some(mut app) = app() else { return };
    let mut bs = mn_core::BalloonSet::new(4.0);
    bs.balloons.push(ellipse(300.0, 300.0, 140.0, 100.0));
    let bl = app.doc.add_balloon_layer("bubble", bs);
    app.tool = Tool::Text;
    app.text_size_pt = 18.0;
    click(&mut app, (330.0, 240.0));
    type_str(&mut app, "そうか…\nわかった");
    esc(&mut app);
    let tl = text_layers(&app)[0];
    let t0 = item(&app, tl, 0).clone();
    render(&mut app, "t09a_balloon_with_text");

    // Move: press inside the balloon but OUTSIDE the text box.
    app.tool = Tool::Object;
    let grab = (300.0 - 110.0, 300.0 + 60.0);
    drag(&mut app, grab, (grab.0 + 200.0, grab.1 + 120.0));
    let b1 = app.doc.layers[bl].balloons().unwrap().balloons[0].clone();
    let BalloonShape::Ellipse { center, .. } = b1.shape else { panic!() };
    assert!((center[0] - 500.0).abs() < 1.0 && (center[1] - 420.0).abs() < 1.0, "{center:?}");
    let t1 = item(&app, tl, 0).clone();
    assert!(
        (t1.pos[0] - t0.pos[0] - 200.0).abs() < 1.0 && (t1.pos[1] - t0.pos[1] - 120.0).abs() < 1.0,
        "the words moved with the bubble: {:?} -> {:?}",
        t0.pos,
        t1.pos
    );
    let img = render(&mut app, "t09b_balloon_moved_with_text");
    assert!(ink_in(&img, [440, 360, 560, 480]) > 0, "text inks inside the moved bubble");

    // Undo takes BOTH back in one press.
    dispatch(&mut app, AppCmd::Undo);
    assert_eq!(item(&app, tl, 0).pos, t0.pos, "one undo, both objects back");
    let BalloonShape::Ellipse { center, .. } =
        app.doc.layers[bl].balloons().unwrap().balloons[0].shape.clone()
    else {
        panic!()
    };
    assert!((center[0] - 300.0).abs() < 1e-3);

    // Resize via the blue box's corner: bottom-right pulled out. The text's
    // centre must sit at the same fraction of the new box.
    app.balloon_sel = Some((bl, 0));
    let b = app.doc.layers[bl].balloons().unwrap().balloons[0].clone();
    let bb = b.bbox();
    let frac_before = {
        let c = item(&app, tl, 0).center();
        [(c[0] - bb[0]) / (bb[2] - bb[0]), (c[1] - bb[1]) / (bb[3] - bb[1])]
    };
    drag(&mut app, (bb[2], bb[3]), (bb[2] + 120.0, bb[3] + 80.0));
    let b2 = app.doc.layers[bl].balloons().unwrap().balloons[0].clone();
    let bb2 = b2.bbox();
    assert!(bb2[2] > bb[2] + 100.0, "grew: {bb:?} -> {bb2:?}");
    let t2 = item(&app, tl, 0).clone();
    let c = t2.center();
    let frac_after = [(c[0] - bb2[0]) / (bb2[2] - bb2[0]), (c[1] - bb2[1]) / (bb2[3] - bb2[1])];
    assert!(
        (frac_after[0] - frac_before[0]).abs() < 0.03 && (frac_after[1] - frac_before[1]).abs() < 0.03,
        "same fraction: {frac_before:?} vs {frac_after:?}"
    );
    assert_eq!(t2.size_pt, t0.size_pt, "type size untouched by the resize");
    render(&mut app, "t09c_balloon_resized_with_text");
}

/// CSP "Changing the color and line thickness": border width, fill
/// opacity 0 (art shows through), screened fill.
#[test]
fn balloon_border_fill_and_screen() {
    let Some(mut app) = app() else { return };
    // A black slab under the bubble to prove the fill covers / reveals.
    let raster = app.doc.active;
    {
        let l = &mut app.doc.layers[raster];
        let mut tiles = std::collections::HashMap::new();
        for ty in 0..12 {
            for tx in 0..14 {
                let idx = mn_core::TileIdx { x: tx, y: ty };
                let mut t = mn_core::Tile::new_transparent();
                let d = t.data_mut();
                for px in d.chunks_exact_mut(4) {
                    px[0] = 0;
                    px[1] = 0;
                    px[2] = 0;
                    px[3] = mn_core::FIX15_ONE as u16;
                }
                tiles.insert(idx, std::sync::Arc::new(t));
            }
        }
        l.replace_tiles(tiles);
    }
    let mut bs = mn_core::BalloonSet::new(6.0);
    bs.balloons.push(ellipse(220.0, 220.0, 120.0, 90.0));
    bs.balloons.push(ellipse(560.0, 220.0, 120.0, 90.0));
    let li = app.doc.add_balloon_layer("bubbles", bs);
    let img = render(&mut app, "t10a_balloons_on_black");
    // Opaque white fill over black.
    let p = img.get_pixel(220, 220);
    assert!(p[0] > 240 && p[1] > 240 && p[2] > 240, "white fill: {p:?}");

    // Fill opacity 0 on the second: black shows through.
    let mut bs = app.doc.layers[li].balloons().unwrap().clone();
    bs.balloons[1].fill_opacity = 0.0;
    bs.balloons[0].line_color = [200, 0, 0];
    bs.border_px = 10.0;
    dispatch(&mut app, AppCmd::BalloonCommit { layer: li, balloons: bs });
    let img = render(&mut app, "t10b_fill_off_border_red");
    let p = img.get_pixel(560, 220);
    assert!(p[0] < 20 && p[1] < 20, "no fill = the art behind: {p:?}");
    let q = img.get_pixel(100, 220); // left rim of bubble 0 (x=100..106)
    assert!(q[0] > 150 && q[1] < 60, "red 10 px border: {q:?}");

    // Screened fill on the first.
    let mut bs = app.doc.layers[li].balloons().unwrap().clone();
    bs.balloons[0].fill_tone = Some(mn_core::balloon::BalloonTone {
        cell_px: 8.0,
        angle_deg: 45.0,
        density: 0.4,
        pattern: mn_core::tone::TonePattern::Dots,
    });
    dispatch(&mut app, AppCmd::BalloonCommit { layer: li, balloons: bs });
    let img = render(&mut app, "t10c_screened_fill");
    let inside = ink_in(&img, [170, 180, 270, 260]);
    assert!(inside > 200 && inside < 100 * 80, "a screen, not paper and not slab: {inside}");
}

/// CSP "Rasterize": Layer ▸ Convert layer with rasterize on turns the
/// text layer into pixels that look identical, and undo brings the text
/// back as text.
#[test]
fn text_to_raster_keeps_the_pixels_and_undoes_to_text() {
    let Some(mut app) = app() else { return };
    app.tool = Tool::Text;
    app.text_size_pt = 22.0;
    click(&mut app, (500.0, 100.0));
    type_str(&mut app, "ラスタライズ");
    esc(&mut app);
    let li = text_layers(&app)[0];
    app.doc.active = li;
    let before = render(&mut app, "t11a_text_before_raster");
    dispatch(
        &mut app,
        AppCmd::ConvertLayer {
            rasterize: true,
            expression: None,
            blend: None,
            keep_original: false,
            name: None,
        },
    );
    assert!(app.doc.layers[li].texts().is_none(), "no longer a text layer");
    let after = render(&mut app, "t11b_text_after_raster");
    assert_eq!(before.as_raw(), after.as_raw(), "pixel-identical bake");
    dispatch(&mut app, AppCmd::Undo);
    assert!(app.doc.layers[li].texts().is_some(), "undo: text again");
}

/// Fit to text: a bubble too small for its words grows around them.
#[test]
fn fit_to_text_grows_a_bubble_around_its_words() {
    let Some(mut app) = app() else { return };
    let mut bs = mn_core::BalloonSet::new(4.0);
    bs.balloons.push(ellipse(300.0, 300.0, 40.0, 30.0));
    let bl = app.doc.add_balloon_layer("bubble", bs);
    app.tool = Tool::Text;
    app.text_size_pt = 20.0;
    click(&mut app, (330.0, 230.0));
    type_str(&mut app, "長い台詞が\nここに入る\nはずだった");
    esc(&mut app);
    render(&mut app, "t12a_bubble_too_small");
    app.fit_balloon_to_text(bl, 0);
    pump(&mut app);
    let b = app.doc.layers[bl].balloons().unwrap().balloons[0].clone();
    let bb = b.bbox();
    let t = item(&app, text_layers(&app)[0], 0);
    let tc = t.corners();
    for c in tc {
        assert!(
            c[0] >= bb[0] - 1.0 && c[0] <= bb[2] + 1.0 && c[1] >= bb[1] - 1.0 && c[1] <= bb[3] + 1.0,
            "text corner {c:?} inside the fitted bubble {bb:?}"
        );
    }
    render(&mut app, "t12b_bubble_fitted");
}

/// A real comic page at print resolution: the default type size is a
/// printable dialogue size (JP 20Q ≈ 14 pt ≈ 5 mm) and the letters land
/// inside the panel where the click was.
#[test]
fn default_text_on_a_600dpi_comic_page_is_dialogue_sized() {
    let Some(mut app) = app() else { return };
    app.new_doc_draft.pages = 1;
    dispatch(&mut app, AppCmd::NewComicCreate);
    let dpi = app.doc_dpi();
    assert!(dpi >= 300, "a comic page is print-res: {dpi}");
    let (w, h) = app.doc.size;
    app.viewport = mn_gpu::Viewport::default();
    app.tool = Tool::Text;
    let at = (w as f32 * 0.6, h as f32 * 0.3);
    click(&mut app, at);
    type_str(&mut app, "ここは\n台詞の\nテスト");
    esc(&mut app);
    let li = text_layers(&app)[0];
    let t = item(&app, li, 0).clone();
    let em_px = mn_text::font_px(&t, dpi);
    let em_mm = em_px / dpi as f32 * 25.4;
    println!("[surface] default {} pt = {em_px:.0} px = {em_mm:.2} mm at {dpi} dpi", t.size_pt);
    assert!(
        (3.0..=6.5).contains(&em_mm),
        "default dialogue em should print 3–6.5 mm, got {em_mm:.2} mm ({} pt)",
        t.size_pt
    );
    // Render a crop around the text at 1:1 for the eye.
    let img = app.renderer.render_offscreen(&app.doc, w, h);
    let bb = ink_bbox(&img).expect("inked");
    let crop = image::imageops::crop_imm(
        &img,
        bb[0].saturating_sub(40),
        bb[1].saturating_sub(40),
        (bb[2] - bb[0] + 80).min(w),
        (bb[3] - bb[1] + 80).min(h),
    )
    .to_image();
    if let Ok(dir) = std::env::var("MN_SURFACE_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        crop.save(format!("{dir}/t13_comic_page_600dpi_crop.png")).unwrap();
    }
}

/// Edge (フチ): a white outline around black type on a black slab is
/// what makes SFX readable over art.
#[test]
fn text_edge_outlines_the_glyphs() {
    let Some(mut app) = app() else { return };
    let raster = app.doc.active;
    {
        let l = &mut app.doc.layers[raster];
        let mut tiles = std::collections::HashMap::new();
        for ty in 0..12 {
            for tx in 0..14 {
                let idx = mn_core::TileIdx { x: tx, y: ty };
                let mut t = mn_core::Tile::new_transparent();
                for px in t.data_mut().chunks_exact_mut(4) {
                    px[3] = mn_core::FIX15_ONE as u16;
                }
                tiles.insert(idx, std::sync::Arc::new(t));
            }
        }
        l.replace_tiles(tiles);
    }
    app.tool = Tool::Text;
    app.text_vertical = false;
    app.text_size_pt = 40.0;
    app.text_outline_mm = 1.0;
    app.text_outline_color = [255, 255, 255];
    click(&mut app, (200.0, 300.0));
    type_str(&mut app, "ドォン");
    esc(&mut app);
    let img = render(&mut app, "t14_edge_on_black");
    // Somewhere in the text area there must be WHITE pixels (the edge).
    let mut white = 0;
    for (_, _, p) in img.enumerate_pixels() {
        if p[0] > 240 && p[1] > 240 && p[2] > 240 {
            white += 1;
        }
    }
    assert!(white > 200, "the フチ inks white on the black: {white}");
}

/// Every `Tail` on a balloon merges into the body: the shared edge is
/// erased, so a spoken tail has no line across its base.
#[test]
fn a_tail_joins_the_body_without_a_seam() {
    let Some(mut app) = app() else { return };
    let mut bs = mn_core::BalloonSet::new(6.0);
    let mut b = ellipse(300.0, 250.0, 150.0, 100.0);
    b.tails.push(Tail {
        base: [300.0, 330.0],
        tip: [340.0, 470.0],
        width: 50.0,
        kind: TailKind::Spoken,
        bend: 0.0,
    });
    bs.balloons.push(b);
    app.doc.add_balloon_layer("bubble", bs);
    let img = render(&mut app, "t15_tail_join");
    // The ellipse bottom at x=300 is y=350; the tail crosses it. A seam
    // would ink at (300, 348..352). The tail interior there must be paper.
    let seam = ink_in(&img, [296, 346, 304, 354]);
    assert_eq!(seam, 0, "no line across the tail's base: {seam} px inked");
}
