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
sits above.** ❌ → roadmap: **clip to a folder**.
Layer A clips to B; the owner groups B and C into a folder so A should clip
to their combined ink. Today the folder header breaks the chain — A's flag
silently ignores and A pops to full visibility. CSP's own answer (folder
becomes the clip target) is the right one, and CSP's *actual* behaviour on
a related edit — the new folder springing into clipped mode itself — is the
annoyance the owner named ("when is that ever useful?"). Ours can never do
that (folders refuse the flag); what is missing is folder-as-base in both
compositors (CPU export walk + GPU canvas walk agree per tile, so this is
a parity feature, not a palette tweak).
Until then, at minimum the app should *say* "clip above lost its base"
instead of changing the picture silently — see 5b.

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

**3d. Merge down a clipped layer onto its base.** 🟡 — the ink is baked
through the clip (merge composites what you saw). Verify: result looks
identical to before the merge.

**3e. Duplicate a clipped layer.** 🟡 — the duplicate carries the flag and
sits inside the run, so it joins it. That is the wanted outcome (duplicate
a shading pass, get a second shading pass on the same base).

## 4. Undo

**4a. Undo any of the above.** 🟡 — clip flags live on the layers and bases
are *derived*, never stored, so undoing a structure change restores the old
meaning automatically. Verify while eye-testing 1–3.

## 5. Feedback the app owes the user

**5a. A clipped layer whose flag is being IGNORED (no valid base)** shows no
differently in the palette today. ❌ — the row should show the clip mark in
a "dangling" state (dimmed / warning tint) so scenario 2a/3a states are
visible at a glance.

**5b. A structure edit that silences or re-bases someone's clip** should say
so in the status line ("Tone A: clip has no base here"). ❌ — cheap, high
value; pairs with 5a.

## Out of scope (decided)

- Folders themselves entering "clipped mode" on creation (the CSP behaviour
  the owner called out as never useful): folders refuse the clip flag here,
  by design. Clip **to** a folder (2a) is the feature; a folder that clips
  is not.
