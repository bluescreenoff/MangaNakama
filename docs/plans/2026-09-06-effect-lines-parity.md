# Plan stub 2026-09-06: effect lines (集中線 / 流線) parity with CSP

Owner, 2026-09-06: "effect lines doesn't seem to have a grouping setting? I might clear then ask you to
implement" and "in general effect lines can be greatly improved compared to CSP, we are behind them 200%".
This stub records what Fable found before the /clear so the next session starts from facts.

## What exists today (facts, file:line)
- Tool-side knobs: `crates/app/src/cmd/tools.rs` `FigureLineOpts` (~line 333): count, width, jitter, r_in_frac
  (focus/flash hole), taper (stream), gap_deg (radial), gap_px + group + group_gap + jit_gap + jit_len +
  jit_width (stream), seed. Presets per sub tool row below it.
- Generator: `crates/core/src/genlines.rs`. **Grouping (まとまり: `group`, `group_gap`) is implemented ONLY in
  the speed-line (stream) walk** (~line 339: bundles of `group` runs at `gap_px`, hole of `group_gap × gap_px`).
  The radial / saturated-line and flash paths do not read `group` at all. That is the missing "grouping
  setting" the owner noticed on 集中線.
- Property panel: `crates/app/src/ui/property/frames_balloons.rs` ~828 (Bundle / Bundle gap bars, stream
  section) and ~965 (a second `group` DragValue, check which tool that section serves).
- Placement: one drag = one generated raster; there is no post-placement object (CSP's effect lines are
  editable objects: move the centre, change the radius/length, re-tune every knob, until rasterized).

## Next session: do this before designing
1. CSP crawl, EN + JP, of the 集中線 / 流線 tool's FULL Tool Property list (JP manual + tool guide:
   「集中線ツール」「流線ツール」 sub tools 集中線 / 流線 / フラッシュ / まばら, and the property groups
   線の本数・間隔 / 長さ / 太さ / グループ化 (本数, 間隔, ばらつき) / 入り抜き・先端形状 / 基準位置・開始位置の
   ばらつき / 描画位置 / ブラシ形状 / アンチエイリアス / 定規として作成 / 集中線オブジェクトの編集ハンドル).
   Cite URLs. Table: CSP knob → ours (exists / partial / missing).
2. Then the plan proper, three tiers: (a) grouping for radial + flash (cheap: the same bundle walk in
   angle space); (b) the missing knobs from the table; (c) effect lines as an editable OBJECT until
   rasterized (the 200 % gap is mostly this), which touches the Object tool, undo, and the .ora vector story
   (`SpeechSet` precedent from Lane 6 for a vector kind that rasterizes on demand).
3. One agent at a time (owner rule 2026-09-05), Fable writes the plan, Opus implements.
