//! Workflow-audit #6: the spring-loaded tool switch. Hold a tool key =
//! borrow (release restores what you had); tap = today's latch, exactly.
//! The arm lives in `main.rs::key_down`, the release in
//! `App::spring_release` — both driven here through the real key path.

use super::new_document_tests::headless;
use crate::cmd::{AppCmd, PanMode, Tool, dispatch};

fn pump(app: &mut crate::app::App) {
    while let Some(c) = app.cmds.pop_front() {
        dispatch(app, c);
    }
}

/// Wind an armed spring past the tap threshold, as if the key had been
/// down for a while.
fn age_spring(app: &mut crate::app::App) {
    let s = app.spring.as_mut().expect("a spring is armed");
    s.at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_millis(300))
        .expect("the process is older than 300ms");
}

const VK_E: u16 = 0x45;
const VK_H: u16 = 0x48;
const VK_R: u16 = 0x52;

/// A quick tap keeps today's meaning exactly: the tool latches.
#[test]
fn a_tap_latches_the_tool() {
    let Some(mut app) = headless() else { return };
    let before = app.tool;
    crate::key_down(&mut app, VK_E, false);
    pump(&mut app);
    assert_ne!(app.tool, before, "E switched the tool");
    let latched = app.tool;
    // Release immediately: under the threshold, no canvas input.
    app.spring_release(VK_E);
    pump(&mut app);
    assert_eq!(app.tool, latched, "a tap is a latch, not a borrow");
    assert!(app.spring.is_none(), "the spring disarmed either way");
}

/// The habit itself: hold, work, release, and you are back — same tool
/// as before the press.
#[test]
fn a_held_key_is_a_borrow() {
    let Some(mut app) = headless() else { return };
    let before = app.tool;
    crate::key_down(&mut app, VK_E, false);
    pump(&mut app);
    assert_ne!(app.tool, before);
    age_spring(&mut app);
    app.spring_release(VK_E);
    pump(&mut app);
    assert_eq!(app.tool, before, "release restored the tool");
}

/// A fast drag under the tap threshold is still a borrow: canvas input
/// while the key is down marks the spring as used.
#[test]
fn a_fast_drag_still_returns() {
    let Some(mut app) = headless() else { return };
    let before = app.tool;
    crate::key_down(&mut app, VK_R, false);
    pump(&mut app);
    assert_eq!(app.tool, Tool::Pan);
    assert_eq!(app.pan_mode, PanMode::Rotate);
    app.begin_pan(100.0, 100.0);
    app.spring_release(VK_R); // released quickly — but the drag happened
    pump(&mut app);
    assert_eq!(app.tool, before, "the fast borrow returned the tool");
}

/// A deliberate tool choice while the key is held wins over the spring:
/// restoring over a palette click would be the spring fighting the hand.
#[test]
fn a_choice_while_held_wins() {
    let Some(mut app) = headless() else { return };
    crate::key_down(&mut app, VK_E, false);
    pump(&mut app);
    dispatch(&mut app, AppCmd::SetTool(Tool::Fill)); // palette click
    age_spring(&mut app);
    app.spring_release(VK_E);
    pump(&mut app);
    assert_eq!(app.tool, Tool::Fill, "the user's choice stood");
}

/// The targeting model (owner ask 2026-08-25) queues `SetSubTool`, not
/// `SetTool` — the spring's tail check learned that shape, so a key bound in
/// `keys.json` to a sub tool springs like a built-in tool key does. Q is
/// deliberately a letter the built-in table does not use.
#[test]
fn a_bound_sub_tool_key_borrows_too() {
    let Some(mut app) = headless() else { return };
    app.keymap = crate::keymap::Keymap::parse(
        r#"{ "q": "tool: Frame border / Cut frame border / Divide frame border" }"#,
    );
    assert!(app.keymap.problems.is_empty(), "{:?}", app.keymap.problems);
    let before = app.tool;
    let mode_before = app.frame_mode;
    crate::key_down(&mut app, 0x51, false);
    assert!(
        matches!(app.cmds.back(), Some(AppCmd::SetSubTool(_))),
        "the binding queued a sub tool: {:?}",
        app.cmds.back()
    );
    pump(&mut app);
    assert_eq!(app.tool, Tool::Frame, "the row's tool is in hand");
    assert_eq!(app.frame_mode, crate::cmd::FrameMode::DivideBorder);
    age_spring(&mut app);
    app.spring_release(0x51);
    pump(&mut app);
    assert_eq!(app.tool, before, "release restored the tool");
    // The borrow restores the TOOL, not the sub tool it left behind — same
    // contract as every other spring (`SpringLoad` saves tool + pan mode).
    assert_ne!(app.frame_mode, mode_before, "the sub tool pick stands");
}

/// H/R pick the Move tool's sub mode as a side effect of the same
/// keydown — a Rotate borrow taken FROM Hand must give Hand back, even
/// though `app.tool` never changed (both are Tool::Pan).
#[test]
fn an_r_borrow_from_hand_restores_the_hand_mode() {
    let Some(mut app) = headless() else { return };
    crate::key_down(&mut app, VK_H, false); // tap-latch the Hand
    pump(&mut app);
    app.spring_release(VK_H);
    assert_eq!((app.tool, app.pan_mode), (Tool::Pan, PanMode::Hand));

    crate::key_down(&mut app, VK_R, false);
    pump(&mut app);
    assert_eq!(app.pan_mode, PanMode::Rotate, "the sub-mode flip armed it");
    age_spring(&mut app);
    app.spring_release(VK_R);
    pump(&mut app);
    assert_eq!(
        (app.tool, app.pan_mode),
        (Tool::Pan, PanMode::Hand),
        "the borrow returned the sub mode with the tool"
    );
}
