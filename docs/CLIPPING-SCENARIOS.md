# Clipping across structure edits — scenario catalog

The owner's ask (2026-08-21): clipping should turn on and off *intelligently*
when the layer structure changes around it. Clip Studio gets several of these
wrong in ways that cost a click and a "why is my art invisible" every time.
This file is the checklist: every scenario is written so it can be verified
by hand in the app, and each carries its current status.

Model recap (ours): `clip` is a per-layer flag, "clip to the layer below".
A **clip run** is a base layer plus the consecutive clipped layers directly
above it at the same depth. The base is the nearest non-clipped, non-folder
layer below at the same depth (`Document::clip_bases`); a clip flag with no
valid base is *ignored*, CSP-style, never an error. Folders refuse the flag
(`set_layer_clip`).

Legend: ✅ handled (test named) · 🟡 handled by accident of the model, verify
by eye · ❌ known gap, on the roadmap · 💬 needs an owner decision.

## 1. Inserting into a clip run

**1a. New layer while the BASE is active.** ✅
Base T with Tone-A, Tone-B clipped above it. Click T, add a layer.
A literal insert above T would land inside the run; the run would re-base
onto the new *empty* layer and every clipped member would go invisible
(alpha of nothing = nothing). Instead the insert **hops above the run**.
If you then clip the new layer, it joins the same run — clip resolves
through the members down to T — so no intent is lost, only the trap.
Pinned by `new_layer_hops_above_the_clip_run` (mn-core).

**1b. New layer while a MID-RUN member is active.** ✅
Same hop, same test. The new layer lands directly above the run's top.

**1c. Paste.** ✅ by construction here — our paste stamps into the active
layer (float → commit) or creates a layer at the TOP of a target folder,
which is above any run inside it. The CSP annoyance (owner's report: paste
above the base of his Pen / Tone(multiply) / Tone(clip) / Tone template
lands inside the run, becomes clipped, and is invisible until you notice)
cannot reproduce in this app. Verify by eye: click a clip base, paste — the
float commits into the base layer itself, nothing vanishes.

**1d. New vector layer / any layer-creating command.** ✅ — every creator
routes through `add_layer` / `add_layer_in_folder`, which hop (1a) or
insert at the folder top respectively.

## 2. Folders around clip structure

**2a. Wrap the base (or base + siblings) in a folder while a clipped layer
sits above.** ✅ — **clip to a folder** shipped.
Layer A clips to B; the owner groups B and C into a folder and A now clips
to their combined ink: a sealed folder header at the same depth is a valid
clip base (`clip_bases`). The base alpha is the group's composite AFTER the
frame mask (panel coverage is part of a frame folder's ink) and BEFORE the
folder's opacity/blend — the same raw-alpha rule layer bases follow — so
turning the folder's opacity down does not thin the clipped layer. A
THROUGH folder has no isolated composite and still breaks the chain; a
hidden folder is zero ink (the clipped layer vanishes — unlike a hidden
*layer* base, whose raw tiles still exist). CSP's annoyance — the new
folder springing into clipped mode itself — remains impossible here
(folders refuse the flag). Pinned by `clip_above_a_folder_resolves_to_the_
folder` (bases), `clip_layer_over_a_folder_clips_to_the_group_ink` (CPU
pixels) and `cpu_matches_gpu_with_clip_to_folder` (GPU parity, including
the empty-group and hidden-folder zero cases).

**2b. Drag a clipped MEMBER into a folder.** 🟡
The member leaves the run; inside the folder its flag re-resolves against
whatever sits below it there, or goes ignored (invisible change, no data
loss). Verify by eye that this matches expectation; the flag is kept, not
cleared, so dragging it back restores the old meaning.

**2c. Dissolve / delete a folder between a clipped layer and a former
base.** 🟡 — the chain re-forms by adjacency (bases recompute every frame).
Verify: delete the folder from 2a's setup; A clips to B again with no
clicks.

## 3. Removing and reordering

**3a. Delete the base.** 🟡 — the run re-bases onto the next non-clipped
layer below, or the flags go ignored (members reappear at full visibility
rather than vanishing). This matches CSP. Verify by eye.

**3b. Drag the base out of the run.** 🟡 — same as 3a at the old spot; at
the new spot the base is just a layer. Members keep their flags.

**3c. Drag a clipped member elsewhere.** 🟡 — it clips to whatever ends up
below it (or ignores). This is CSP behaviour and is *usually* wanted
(moving a shading layer onto a different base re-clips it there). 💬 If
eye-testing says this surprises more than it helps, the alternative is
clearing the flag on any drag that changes the base — owner's call.

**3d. Merge down a clipped layer onto its base.** ✅ — it **refuses**.
`merge_down` bails on a clipped upper layer because "a clipped layer's raw
pixels are not what it shows": merging copies the layer's own tiles, so the
part the clip was hiding would come back as ink. Nothing is baked and
nothing is lost — unclip first if you want the merge. Pinned by
`alpha_lock_masks_the_open_op_and_locks_guard_merge` (mn-core).

**3e. Duplicate a clipped layer.** 🟡 — the duplicate carries the flag and
sits inside the run, so it joins it. That is the wanted outcome (duplicate
a shading pass, get a second shading pass on the same base).

## 4. Undo

**4a. Undo any of the above.** 🟡 — clip flags live on the layers and bases
are *derived*, never stored, so undoing a structure change restores the old
meaning automatically. Verify while eye-testing 1–3.

## 5. Feedback the app owes the user

**5a. A clipped layer whose flag is being IGNORED (no valid base)** ✅ —
the palette row's clip bar dims from red to grey (`theme::TEXT_WEAK`) when
the flag has no base under it, so scenario 2a/3a states read at a glance.
The dangling test is `clip && !folder && clip_bases[i].is_none()`, recomputed
per frame in `ui::layers`.

**5b. A structure edit that silences or re-attaches someone's clip** ✅ —
the status line says so: `"Shade": clip lost its base here — the flag is
ignored (grey mark)` / `"Shade": clip re-attached to what sits below`.
`cmd::dispatch` snapshots which clipped layers are dangling before every
command and compares after, keyed by NAME (indices shift under exactly
these edits; a name absent on either side never reports, so renames and
deletions stay quiet). Pinned by
`a_structure_edit_that_silences_a_clip_reports_it`.

## Out of scope (decided)

- Folders themselves entering "clipped mode" on creation (the CSP behaviour
  the owner called out as never useful): folders refuse the clip flag here,
  by design. Clip **to** a folder (2a) is the feature; a folder that clips
  is not.
