use super::new_document_tests::headless;
use crate::app::App;

fn typed(app: &App) -> String {
    app.edited_item()
        .map(|i| i.text.clone())
        .unwrap_or_default()
}

fn type_str(app: &mut App, s: &str) {
    for u in s.encode_utf16() {
        app.text_char(u);
    }
}

/// The owner, 2026-08-19: *"Ctrl+Z seems to remove an entire text rather
/// than edit something in it."* It did — the session was one undo step.
/// Now it walks back through the edits first, in word-sized bites.
#[test]
fn ctrl_z_steps_back_through_typing_before_it_removes_the_box() {
    let Some(mut app) = headless() else { return };
    app.start_new_text([300.0, 300.0], None);
    assert!(app.text_editing(), "the session is open");

    type_str(&mut app, "ab cd");
    assert_eq!(typed(&app), "ab cd");

    // A run of characters is ONE step, not five.
    assert!(app.text_key(0x5A, true, false));
    assert_eq!(typed(&app), "ab ", "the last word went, not the letter");

    assert!(app.text_key(0x5A, true, false));
    assert_eq!(typed(&app), "ab", "whitespace is its own step");

    assert!(app.text_key(0x5A, true, false));
    assert_eq!(typed(&app), "", "back to the empty box");

    // Stack empty: the next Ctrl+Z ends the session and hands over to the
    // document's undo — the right LAST step, and the only one there used
    // to be.
    assert!(app.text_key(0x5A, true, false));
    assert!(!app.text_editing(), "the session closed");
}

/// A deletion is its own step, and undoing it puts the text back rather
/// than merging with the typing around it.
#[test]
fn a_deletion_undoes_on_its_own() {
    let Some(mut app) = headless() else { return };
    app.start_new_text([300.0, 300.0], None);
    type_str(&mut app, "hello");
    app.text_key(0x08, false, false); // Backspace
    app.text_key(0x08, false, false);
    assert_eq!(typed(&app), "hel");

    assert!(app.text_key(0x5A, true, false));
    assert_eq!(typed(&app), "hell", "one backspace back");
    assert!(app.text_key(0x5A, true, false));
    assert_eq!(typed(&app), "hello", "and the other");
}
