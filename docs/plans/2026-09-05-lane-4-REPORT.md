# Lane 4 report — process priority (H), touch rotate activation (I)

## Done / next
- **done:** H — `priority.rs`, call from `shell.rs`, one `mod priority;` line in `main.rs`.
- **done:** I — `app.rs` two activation gates, tests edited + one new, `drawing.html` paragraph.
- **done:** gate — `cargo check --workspace --all-targets` clean (zero warnings), all six tests pass.
- **next:** nothing in this lane. app.rs was reverted once by the coordinator and re-applied
  byte-identical; discriminating run confirmed by Fable (dominance gate = 0 makes the new test fail).

## H — process priority

**Answer to the owner's question ("is our priority low? should we raise it in all builds?").**
Yes, we ran at NORMAL like every other process, and yes, raise it — but it fixes only part of
what he felt. Two things make the G-pen lag while two agent sessions run:

1. **CPU contention.** A background `cargo` build has several worker threads at NORMAL priority,
   exactly the same priority as our window thread. Windows gives them equal turns, so the thread
   that reads the pen and paints the stroke waits behind a compiler. This is what a higher
   priority class fixes.
2. **RAM starvation.** The machine has 15.8 GB and is running two agent sessions plus cargo plus
   the app. When memory runs out Windows pages to disk, and a thread that hits a page fault waits
   for the disk *no matter how important it is*. Priority does nothing here. So expect an
   improvement, not a cure — the swapping half needs fewer things running, not a bigger number.

**ABOVE_NORMAL, never HIGH or REALTIME.** Above HIGH we would outrank the digitizer driver's own
threads and the DWM compositor; starving those makes pen input *worse*, which is the opposite of
the request. Brush/dab worker threads are deliberately left at NORMAL — putting the UI thread
above them is the whole point.

**Implementation.** `crates/app/src/priority.rs::raise_for_interactive()` calls
`SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS)` +
`SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL)`. Called once from
`Shell::new` (`crates/app/src/shell.rs`), which runs on the window thread — the thread that pumps
`WM_POINTER` and paints. Always on, no preference (there is no existing preference for system
knobs like this).

Outcome message, printed once at startup on the `[app]` log line:

- success: `[app] priority: above-normal`
- failure: `[app] priority: normal — SetPriorityClass ok=<bool>, SetThreadPriority ok=<bool>, GetLastError <n>`

(The plan said "status/log line". The status line belongs to `App`, which does not exist yet when
`Shell::new` runs, and `app.rs`'s status code is outside this lane's owned region — so it goes to
the `[app]` stdout log, the same channel `CreateWindowExW failed` and `restoring window at …` use.)

**Two cfg guards.** `#[cfg(windows)]` per the plan (no-op elsewhere), and **`not(test)`**: a
`cargo test` run must not raise *itself* above the app the owner is drawing in. `Shell::new` is
called by `App::new`, which every headless test calls.

**`mod priority;` had to go in `main.rs`.** `main.rs` is the crate root of the `mn-app` binary
(there is no `lib.rs`), so it is the only place a top-level module can be declared. Added exactly
one line, in the alphabetical `mod` list at the top (line ~25, between `mod keymap;` and
`mod recovery;`) — nowhere near `builtin_chords` / `builtin_targets` / `shortcut`, which Lane 1
owns. Nothing else in `main.rs` was touched.

## I — touch rotate activation

**Constants (in `crates/app/src/app.rs`):**

| constant | old | new |
| --- | --- | --- |
| `TOUCH_TWIST_THRESHOLD` | `4.0f32.to_radians()` | `12.0f32.to_radians()` |
| `TOUCH_TWIST_DOMINANCE` | — (new) | `40.0f32.to_radians()` |

New field `TouchTwist::pinch: f32` — the running sum of `|ln(distance ratio)|` over every touch
event since the pair formed (the same clamped ratio that is fed to `zoom_around`, so the
accumulator and the view agree). Reset with the rest of the accumulator on any contact-set change.

The latch now needs **both** gates:

```rust
if !g.live
    && g.raw.abs() >= TOUCH_TWIST_THRESHOLD          // 12° of cumulative twist
    && g.raw.abs() >= TOUCH_TWIST_DOMINANCE * g.pinch // and the twist out-argues the pinch
{ g.live = true; }
```

Everything after the latch is untouched: the accumulated twist is applied on engage (`target =
start_rad + raw`), so nothing the fingers did is lost, and the quarter-snap hysteresis
(`SNAP_ENGAGE` 2.5°, `SNAP_RELEASE` 4°) is exactly as before. Pan and zoom still track from the
first event.

### Why 12° and 40°/e-fold

`TOUCH_TWIST_DOMINANCE` reads as *degrees of twist required per unit of accumulated `|ln(scale)|`*.
40° per e-fold means:

- a **pure twist** (< 5 % scale drift) needs `40 × ln(1.05) = 1.95°` from gate 2 — so gate 1's 12°
  is what it actually meets, exactly as the plan asks;
- a **±25 % pinch** demands `40 × ln(1.25) = 8.9°` of twist before rotation may start, and a wobble
  of ±3° is nowhere near it — and gate 1's 12° blocks it a second time;
- a **2× zoom** demands `40 × ln(2) = 27.7°`, more than a hand drifts while spreading two fingers.

Checked against every existing test before running them (numbers from the pair geometry the tests
drive, where each `touch_move` is a half-update against the static other finger):

- `two_finger_rotate_snaps_to_quarters`: first half-event is a 15° twist with 0.035 of pinch
  (the half-update shortens the pair by 3.4 %) → needs 1.4°, has 15°. Engages on event 1, same as
  the old 4° code.
- `two_finger_slow_twist_rotates`: 1° steps, pinch artifact 3.8e-5 per event → gate 2 asks for
  0.27° at the end. Engages at 12° of twist (step 12) and the accumulated 12° is applied, so the
  gesture still lands on the quarter. This is the test the raised threshold most endangered; it
  survives *because* engage applies the backlog.
- `two_finger_pinch_twist_is_one_gesture` (1.5× spread + 20° turn): gate 2 asks
  `40 × ln(1 + 0.025k)` at step k, gate 1 asks 12°. Both are met around step 12–13 of 20, so the
  full 20° still arrives.
- `two_finger_snap_holds_then_releases_at_any_speed`: pure twist, engages at 12°, snap logic
  untouched.
- `two_finger_pinch_noise_does_not_rotate`, strengthened per the plan: pinch-out to 1.5× with the
  grip **slipping ±25 %** each step and the pair angle wobbling **±3°**. Cumulative twist never
  leaves ±3°, so gate 1 alone stops it; final zoom 1.875× still passes the "zoom keeps tracking"
  assertion.
- new `a_wobbly_zoom_never_starts_a_rotate`: the owner's actual repro — a 2× spread while the whole
  hand **drifts 12°** with ±3° of wobble. Peak twist 15°, which clears the old 4° gate *and* the
  new 12° gate, so only the dominance gate keeps the page straight: at the moment twist reaches 12°
  the pinch has already piled up `40 × ln(1.75) = 22.4°` of requirement. Without
  `TOUCH_TWIST_DOMINANCE` this test fails, which is what makes it worth having.

### Test results (2026-09-05)

`cargo check --workspace --all-targets` — clean, zero warnings.

`cargo test -p mn-app two_finger` — 8 passed, 0 failed:
- `app::tests::two_finger_slow_twist_rotates`
- `app::tests::two_finger_rotate_snaps_to_quarters`
- `app::tests::two_finger_pinch_twist_is_one_gesture`
- `app::tests::two_finger_snap_holds_then_releases_at_any_speed`
- `app::tests::two_finger_pinch_noise_does_not_rotate` (strengthened: ±25 % grip slip, ±3° wobble)
- (plus the three `gesture::tests::*_two_finger_*` tap tests the filter also matches, all ok)

`cargo test -p mn-app a_wobbly_zoom_never_starts_a_rotate` — 1 passed.

**Discriminating run: DONE (by Fable).** With `TOUCH_TWIST_DOMINANCE` set to 0 — leaving only the 12°
gate — `a_wobbly_zoom_never_starts_a_rotate` FAILS, so the dominance gate is the thing holding that
test up, exactly as the arithmetic above predicts (its twist peaks at 15°, above 12°).

**Re-apply note (same day).** `crates/app/src/app.rs` was reverted to HEAD by an accidental
`git checkout -- app.rs` on the coordinator's side; the item I hunks were re-applied byte-identical
(37 insertions, 4 deletions, 5 hunks: TouchTwist doc + `pinch` field, the two constants, the `ratio`
binding, the two-gate latch). `cargo check -p mn-app --all-targets` clean, all nine matching tests
pass again.
