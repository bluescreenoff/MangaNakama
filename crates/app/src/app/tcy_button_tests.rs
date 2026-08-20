use super::new_document_tests::headless;
use crate::app::App;

fn type_str(app: &mut App, s: &str) {
    for u in s.encode_utf16() {
        app.text_char(u);
    }
}

fn select(app: &mut App, a: u32, b: u32) {
    if let Some(ed) = app.text_edit.as_mut() {
        ed.anchor = a;
        ed.caret = b;
    }
}

/// The model shipped before any way to reach it. This is the control:
/// select the number, press once to stand it upright, press again to put
/// it back — the B/I/U shape, because that is what it looks like.
#[test]
fn the_button_toggles_the_selection_upright_and_back() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    app.start_new_text([300.0, 300.0], None);
    type_str(&mut app, "22時に");
    select(&mut app, 0, 2);

    assert!(!app.selection_is_tcy(), "nothing is upright yet");
    app.text_tcy_button();
    assert!(app.selection_is_tcy(), "the digits stand up");
    assert_eq!(
        app.edited_item().map(|i| i.tcy.len()),
        Some(1),
        "as one run, not two"
    );

    app.text_tcy_button();
    assert!(!app.selection_is_tcy(), "and lie back down");
    assert_eq!(app.edited_item().map(|i| i.tcy.is_empty()), Some(true));
}

/// A partially-covered selection is not "on": pressing must finish the
/// job rather than undo half of it. This is the case a naive
/// `any()` check gets backwards.
#[test]
fn a_partly_upright_selection_reads_as_off_and_the_press_completes_it() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    app.start_new_text([300.0, 300.0], None);
    type_str(&mut app, "12345");

    select(&mut app, 0, 2);
    app.text_tcy_button();
    select(&mut app, 0, 4);
    assert!(
        !app.selection_is_tcy(),
        "half-covered must read as off, or the press would undo the half that is on"
    );
    app.text_tcy_button();
    assert!(app.selection_is_tcy(), "the whole selection is upright now");
}

/// With no selection the button says what to do instead of quietly
/// doing nothing — the rule the drop router already follows.
#[test]
fn with_no_selection_it_explains_itself() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    app.start_new_text([300.0, 300.0], None);
    type_str(&mut app, "22時");
    select(&mut app, 1, 1);
    app.text_tcy_button();
    assert!(
        app.status.contains("select"),
        "status should ask for a selection, got {:?}",
        app.status
    );
    assert_eq!(app.edited_item().map(|i| i.tcy.is_empty()), Some(true));
}

/// It rides the in-editor undo stack like every other edit — one press,
/// one Ctrl+Z.
#[test]
fn one_press_is_one_undo_step() {
    let Some(mut app) = headless() else {
        println!("[test] SKIP: no usable adapter");
        return;
    };
    app.start_new_text([300.0, 300.0], None);
    type_str(&mut app, "22時");
    select(&mut app, 0, 2);
    app.text_tcy_button();
    assert!(app.selection_is_tcy());

    assert!(app.text_key(0x5A, true, false), "Ctrl+Z");
    assert_eq!(
        app.edited_item().map(|i| i.tcy.is_empty()),
        Some(true),
        "the upright run came back down in one step"
    );
    assert_eq!(
        app.edited_item().map(|i| i.text.clone()),
        Some("22時".to_owned()),
        "and the text itself was not touched"
    );
}
