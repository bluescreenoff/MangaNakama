use super::*;

/// The values the text sections edit: the item under edit / the Object
/// selection, falling back to the new-text defaults.
pub(crate) struct TextState {
    font: String,
    pt: f32,
    vert: bool,
    edge_mm: f32,
    align: mn_core::text::Align,
    frame_align: mn_core::text::FrameAlign,
    letter_pt: f32,
    line: mn_core::text::LineSpacing,
}

pub(crate) fn text_state(app: &App) -> TextState {
    let dpi = app.doc_dpi().max(96);
    let px_to_mm = |px: f32| px * 25.4 / dpi as f32;
    match crate::text_edit::property_target(app)
        .and_then(|(li, ti)| app.doc.layers.get(li)?.texts()?.texts.get(ti).cloned())
    {
        Some(item) => TextState {
            font: item.font.clone(),
            pt: item.size_pt,
            vert: item.vertical,
            edge_mm: px_to_mm(item.outline_px),
            align: item.align,
            frame_align: item.frame_align,
            letter_pt: item.letter_spacing_pt,
            line: item.line_spacing,
        },
        None => TextState {
            font: app.text_font.clone(),
            pt: app.text_size_pt,
            vert: app.text_vertical,
            edge_mm: app.text_outline_mm,
            align: app.text_align,
            frame_align: app.text_frame_align,
            letter_pt: app.text_letter_pt,
            line: app.text_line,
        },
    }
}

pub(crate) fn sec_text_font(ui: &mut egui::Ui, app: &mut App) {
    let st = text_state(app);
    // Font family — CSP's Font list shape: the button opens an inline panel
    // with search, Recently used (max 10) and every installed family.
    if ui
        .button(format!(
            "Font ▾  {}",
            if st.font.is_empty() {
                "(default)"
            } else {
                &st.font
            }
        ))
        .clicked()
    {
        app.font_picker_open = !app.font_picker_open;
    }
    if app.font_picker_open {
        let families: Vec<String> = app
            .text_engine
            .as_ref()
            .map(|e| e.families().to_vec())
            .unwrap_or_default();
        let mut search = app.font_search.clone();
        ui.text_edit_singleline(&mut search);
        app.font_search = search;
        let mut picked: Option<String> = None;
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .id_salt("mn.text.font.list")
            .show(ui, |ui| {
                let q = app.font_search.to_lowercase();
                let hits = |f: &str| q.is_empty() || f.to_lowercase().contains(&q);
                if app.recent_fonts.iter().any(|f| hits(f)) {
                    ui.weak("Recently used");
                    for f in app.recent_fonts.iter().filter(|f| hits(f)) {
                        if ui.selectable_label(*f == st.font, f).clicked() {
                            picked = Some(f.clone());
                        }
                    }
                    ui.separator();
                }
                ui.weak("All fonts");
                for f in families.iter().filter(|f| hits(f)) {
                    if ui.selectable_label(*f == st.font, f).clicked() {
                        picked = Some(f.clone());
                    }
                }
            });
        if let Some(f) = picked {
            app.note_recent_font(&f);
            app.font_picker_open = false;
            app.font_search.clear();
            // With characters selected the pick applies to THEM (TX-064 —
            // the Latin word inside a Japanese balloon), exactly like the
            // B/I/U rule one section down. With no selection it is the
            // item's font, and it also becomes the default for new text.
            if app.text_edit.as_ref().is_some_and(|ed| ed.has_selection()) {
                app.text_font_range_button(f);
            } else {
                app.text_font = f.clone();
                app.apply_text_prop(move |i| i.font = f.clone());
            }
        }
    }
    let mut pt = st.pt;
    let resp = ValueBar::new("Size", 4.0, 72.0)
        .decimals(1)
        .suffix(" pt")
        .log()
        .show(ui, &mut pt);
    if resp.changed() {
        app.begin_text_bar_drag();
        app.text_size_pt = pt;
        app.preview_text_prop(move |i| i.size_pt = pt);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        app.commit_text_bar_drag();
    }
    // Character spacing (CSP Font group): pt at the document dpi, negative
    // tightens.
    let mut ls = st.letter_pt;
    let resp = ValueBar::new("Char space", -3.0, 6.0)
        .decimals(2)
        .suffix(" pt")
        .show(ui, &mut ls);
    if resp.changed() {
        app.begin_text_bar_drag();
        app.text_letter_pt = ls;
        app.preview_text_prop(move |i| i.letter_spacing_pt = ls);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        app.commit_text_bar_drag();
    }
}

/// Furigana (TX-062). Select the kanji in the text, type the reading, press
/// ルビ. The field's hint shows the reading already under the caret, so an
/// existing annotation is visible without a round trip through the canvas.
pub(crate) fn sec_text_ruby(ui: &mut egui::Ui, app: &mut App) {
    let editing = app.text_editing();
    let has_sel = app.text_edit.as_ref().is_some_and(|ed| ed.has_selection());
    let at_caret = app.ruby_at_caret();
    ui.horizontal(|ui| {
        let hint = at_caret.clone().unwrap_or_else(|| "よみ".to_owned());
        let field = egui::TextEdit::singleline(&mut app.text_ruby)
            .hint_text(hint)
            .desired_width(ui.available_width() - 46.0);
        let resp = ui.add_enabled(editing, field);
        let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let pressed = ui
            .add_enabled(has_sel, egui::Button::new("ルビ"))
            .on_hover_text(
                "Set the reading over the selected characters.\n\
                 An empty field clears it. Vertical text sets the reading on \
                 the right of the column, horizontal above the word.",
            )
            .clicked();
        if pressed || (entered && has_sel) {
            app.text_ruby_button();
        }
    });
    if !editing {
        ui.label(
            egui::RichText::new("double-click the text to set furigana")
                .weak()
                .size(10.0),
        );
    } else if !has_sel {
        ui.label(
            egui::RichText::new("select the kanji first")
                .weak()
                .size(10.0),
        );
    }

    // CSP's "Reading settings" (owner: furigana "needs a lot of settings you
    // can access like Clip Studio's"). These are per TEXT ITEM, not per
    // reading — CSP's are too, and a page where two readings in one balloon
    // are set differently is a page with a mistake on it.
    let st = crate::text_edit::property_target(app)
        .and_then(|(li, ti)| app.doc.layers.get(li)?.texts()?.texts.get(ti))
        .map(|i| i.ruby_style.clone())
        .unwrap_or_default();

    let mut size_pct = st.size_pct;
    let resp = ValueBar::new("Size", 20.0, 100.0)
        .decimals(0)
        .suffix(" %")
        .show(ui, &mut size_pct);
    if resp.changed() {
        app.begin_text_bar_drag();
        app.preview_text_prop(move |i| i.ruby_style.size_pct = size_pct);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        app.commit_text_bar_drag();
    }

    let mut gap = st.gap_pt;
    let resp = ValueBar::new("Gap", -2.0, 6.0)
        .decimals(2)
        .suffix(" pt")
        .show(ui, &mut gap);
    if resp.changed() {
        app.begin_text_bar_drag();
        app.preview_text_prop(move |i| i.ruby_style.gap_pt = gap);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        app.commit_text_bar_drag();
    }

    let mut adjust = st.offset_pt;
    let resp = ValueBar::new("Adjust", -12.0, 12.0)
        .decimals(2)
        .suffix(" pt")
        .show(ui, &mut adjust);
    if resp.changed() {
        app.begin_text_bar_drag();
        app.preview_text_prop(move |i| i.ruby_style.offset_pt = adjust);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        app.commit_text_bar_drag();
    }

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Along")
                .size(10.5)
                .color(theme::TEXT_WEAK),
        );
        let mut align = st.align;
        for (label, value, tip) in [
            (
                "Start",
                mn_core::text::Align::Leading,
                "against the start of the word",
            ),
            (
                "Center",
                mn_core::text::Align::Center,
                "centred on the word (the usual setting)",
            ),
            (
                "End",
                mn_core::text::Align::Trailing,
                "against the end of the word",
            ),
        ] {
            if ui
                .selectable_label(align == value, label)
                .on_hover_text(tip)
                .clicked()
                && align != value
            {
                align = value;
                app.apply_text_prop(move |i| i.ruby_style.align = value);
            }
        }
    });
}

pub(crate) fn sec_text_align(ui: &mut egui::Ui, app: &mut App) {
    let st = text_state(app);
    // Row alignment (CSP "Alignment") + block position in the frame (CSP
    // "Position in frame"). Labels follow the orientation exactly like
    // CSP's palette; Leading = the reading start (left / top).
    let mut row_pick: Option<mn_core::text::Align> = None;
    let mut frame_pick: Option<mn_core::text::FrameAlign> = None;
    ui.horizontal(|ui| {
        ui.weak("Rows");
        let names: [&str; 3] = if st.vert {
            ["Top", "Center", "Bottom"]
        } else {
            ["Left", "Center", "Right"]
        };
        for (a, label) in [
            (mn_core::text::Align::Leading, names[0]),
            (mn_core::text::Align::Center, names[1]),
            (mn_core::text::Align::Trailing, names[2]),
        ] {
            if ui.selectable_label(st.align == a, label).clicked() {
                row_pick = Some(a);
            }
        }
    });
    ui.horizontal(|ui| {
        ui.weak("In frame");
        let names: [&str; 3] = if st.vert {
            ["Right", "Center", "Left"]
        } else {
            ["Top", "Center", "Bottom"]
        };
        for (a, label) in [
            (mn_core::text::FrameAlign::Near, names[0]),
            (mn_core::text::FrameAlign::Center, names[1]),
            (mn_core::text::FrameAlign::Far, names[2]),
        ] {
            if ui.selectable_label(st.frame_align == a, label).clicked() {
                frame_pick = Some(a);
            }
        }
    })
    .response
    .on_hover_text("Where the text block sits in the wrap box (CSP: Position in frame)");
    if let Some(a) = row_pick {
        app.text_align = a;
        app.apply_text_prop(move |i| i.align = a);
    }
    if let Some(a) = frame_pick {
        app.text_frame_align = a;
        app.apply_text_prop(move |i| i.frame_align = a);
    }
}

pub(crate) fn sec_text_spacing(ui: &mut egui::Ui, app: &mut App) {
    let st = text_state(app);
    // Line space + "How to specify" (CSP L-row): Auto = the font's own
    // metrics; a percentage of the natural line height; or an absolute pt.
    let mut mode = match st.line {
        mn_core::text::LineSpacing::Auto => 0u8,
        mn_core::text::LineSpacing::Percent(_) => 1,
        mn_core::text::LineSpacing::Pt(_) => 2,
    };
    let mut mode_pick: Option<u8> = None;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut mode, 0, "Auto");
        ui.selectable_value(&mut mode, 1, "%");
        ui.selectable_value(&mut mode, 2, "pt");
        if mode
            != match st.line {
                mn_core::text::LineSpacing::Auto => 0,
                mn_core::text::LineSpacing::Percent(_) => 1,
                mn_core::text::LineSpacing::Pt(_) => 2,
            }
        {
            mode_pick = Some(mode);
        }
    });
    if let Some(m) = mode_pick {
        let ls = match m {
            0 => mn_core::text::LineSpacing::Auto,
            1 => mn_core::text::LineSpacing::Percent(100.0),
            // Absolute seed ≈ the common 1.3× line for the current size.
            _ => mn_core::text::LineSpacing::Pt((st.pt * 1.3).max(1.0)),
        };
        app.text_line = ls;
        app.apply_text_prop(move |i| i.line_spacing = ls);
        return;
    }
    match st.line {
        mn_core::text::LineSpacing::Percent(v) => {
            let mut val = v;
            let resp = ValueBar::new("Line", 50.0, 300.0)
                .decimals(0)
                .suffix(" %")
                .show(ui, &mut val);
            if resp.changed() {
                app.begin_text_bar_drag();
                app.text_line = mn_core::text::LineSpacing::Percent(val);
                app.preview_text_prop(move |i| {
                    i.line_spacing = mn_core::text::LineSpacing::Percent(val)
                });
            }
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                app.commit_text_bar_drag();
            }
        }
        mn_core::text::LineSpacing::Pt(v) => {
            let mut val = v;
            let resp = ValueBar::new("Line", 5.0, 150.0)
                .decimals(1)
                .suffix(" pt")
                .show(ui, &mut val);
            if resp.changed() {
                app.begin_text_bar_drag();
                app.text_line = mn_core::text::LineSpacing::Pt(val);
                app.preview_text_prop(move |i| {
                    i.line_spacing = mn_core::text::LineSpacing::Pt(val)
                });
            }
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                app.commit_text_bar_drag();
            }
        }
        mn_core::text::LineSpacing::Auto => {}
    }
}

pub(crate) fn sec_text_dir(ui: &mut egui::Ui, app: &mut App) {
    let st = text_state(app);
    ui.horizontal(|ui| {
        let mut vert = st.vert;
        ui.selectable_value(&mut vert, true, "縦書き");
        ui.selectable_value(&mut vert, false, "横書き");
        if vert != st.vert {
            app.text_vertical = vert;
            app.apply_text_prop(move |i| i.vertical = vert);
        }
    });
    sec_text_auto_tcy(ui, app);
}

/// Auto 縦中横 (TX-062) — CSP's "Advanced ▸ Text ▸ Auto TateChuYoko", the
/// dropdown that decides how long a run of half-width alphanumerics may be
/// and still be stood upright without anyone selecting it.
///
/// It sits under the direction switch because that is the setting it depends
/// on: 縦中横 means "horizontal inside vertical", so in horizontal text this
/// is greyed rather than quietly doing nothing.
///
/// The value is per ITEM (a balloon of dialogue and a page number want
/// different answers) and doubles as the default for new text, exactly like
/// the alignment and spacing controls one section down.
pub(crate) fn sec_text_auto_tcy(ui: &mut egui::Ui, app: &mut App) {
    let vertical = text_state(app).vert;
    let current = crate::text_edit::property_target(app)
        .and_then(|(li, ti)| app.doc.layers.get(li)?.texts()?.texts.get(ti))
        .map(|i| i.auto_tcy)
        .unwrap_or(app.text_auto_tcy);
    let mut pick: Option<u8> = None;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("自動縦中横")
                .size(10.5)
                .color(theme::TEXT_WEAK),
        );
        for n in 0..=mn_core::text::AUTO_TCY_MAX {
            let label = if n == 0 {
                "off".to_owned()
            } else {
                n.to_string()
            };
            let tip = if n == 0 {
                "Leave every alphanumeric run as it falls — only the spans \
                 you mark with 縦中横 stand upright."
                    .to_owned()
            } else {
                format!(
                    "Stand runs of up to {n} half-width alphanumerics upright \
                     on their own: 第1話, 22時, 3年.\nA longer run is left \
                     lying down rather than squeezed into one cell."
                )
            };
            if ui
                .add_enabled(
                    vertical,
                    egui::Button::new(egui::RichText::new(label).size(11.0)).selected(current == n),
                )
                .on_hover_text(tip)
                .clicked()
                && current != n
            {
                pick = Some(n);
            }
        }
    });
    if let Some(n) = pick {
        app.text_auto_tcy = n;
        app.apply_text_prop(move |i| i.auto_tcy = n);
    }
}

pub(crate) fn sec_text_style(ui: &mut egui::Ui, app: &mut App) {
    // B / I / U on the current selection while editing; on the whole item
    // otherwise.
    ui.horizontal(|ui| {
        let can = app.text_edit.as_ref().is_some_and(|ed| ed.has_selection());
        for (label, flag, tip) in [
            ("B", mn_core::StyleFlag::Bold, "Bold (Ctrl+B)"),
            ("I", mn_core::StyleFlag::Italic, "Italic (Ctrl+I)"),
            ("U", mn_core::StyleFlag::Underline, "Underline (Ctrl+U)"),
            (
                "S",
                mn_core::StyleFlag::Strike,
                "Strikethrough (CSP style row; works in vertical text too)",
            ),
        ] {
            let text = match flag {
                mn_core::StyleFlag::Bold => egui::RichText::new(label).strong(),
                mn_core::StyleFlag::Italic => egui::RichText::new(label).italics(),
                mn_core::StyleFlag::Underline => egui::RichText::new(label).underline(),
                mn_core::StyleFlag::Strike => egui::RichText::new(label).strikethrough(),
            };
            if ui
                .add_enabled(
                    can || !app.text_editing(),
                    egui::Button::new(text.size(12.0)),
                )
                .on_hover_text(tip)
                .clicked()
            {
                app.text_style_button(flag);
            }
        }
        // 縦中横 (TX-063) sits with B/I/U because it behaves like them: a
        // toggle over the selected characters, pressed when they are already
        // upright. It only means anything in vertical text, so it is
        // disabled in horizontal — the setting exists there in the model but
        // the engine ignores it, and an enabled control that does nothing is
        // worse than a greyed one.
        let vertical = text_state(app).vert;
        let is_on = app.selection_is_tcy();
        if ui
            .add_enabled(
                vertical && can,
                egui::Button::new(egui::RichText::new("縦中横").size(11.0)).selected(is_on),
            )
            .on_hover_text(
                "Stand the selected characters upright inside the column.\n\
                 How numbers are set in vertical Japanese — 22時, not a 2 and\n\
                 a 2 lying on their sides. Vertical text only.",
            )
            .clicked()
        {
            app.text_tcy_button();
        }
    });
    // Text colour (CSP ties it to the drawing colour).
    ui.horizontal(|ui| {
        let c = app.active_color();
        let rgb = egui::Color32::from_rgb(
            (c[0] * 255.0) as u8,
            (c[1] * 255.0) as u8,
            (c[2] * 255.0) as u8,
        );
        let (sw, _) = ui.allocate_exact_size(egui::vec2(13.0, 13.0), egui::Sense::hover());
        ui.painter().rect_filled(sw, 2.0, rgb);
        if ui.button("Drawing colour").clicked() {
            let b = [
                (c[0] * 255.0) as u8,
                (c[1] * 255.0) as u8,
                (c[2] * 255.0) as u8,
            ];
            app.apply_text_prop(move |i| i.color = b);
        }
        for (b, label) in [([0u8, 0, 0], "Black"), ([255u8, 255, 255], "White")] {
            if ui.selectable_label(false, label).clicked() {
                app.apply_text_prop(move |i| i.color = b);
            }
        }
    });
}

pub(crate) fn sec_text_edge(ui: &mut egui::Ui, app: &mut App) {
    let st = text_state(app);
    let dpi = app.doc_dpi().max(96);
    let mut edge = st.edge_mm;
    let resp = ValueBar::new("Edge (フチ)", 0.0, 2.0)
        .decimals(2)
        .suffix(" mm")
        .show(ui, &mut edge);
    if resp.changed() {
        app.begin_text_bar_drag();
        app.text_outline_mm = edge;
        let px = edge / 25.4 * dpi as f32;
        app.preview_text_prop(move |i| i.outline_px = px.max(0.0));
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        app.commit_text_bar_drag();
    }
    ui.horizontal(|ui| {
        ui.weak("edge colour");
        for (c, label) in [([255u8, 255, 255], "White"), ([0, 0, 0], "Black")] {
            if ui.selectable_label(false, label).clicked() {
                app.text_outline_color = c;
                app.apply_text_prop(move |i| i.outline_color = c);
            }
        }
    });
}

/// TX-styles: the WORK style row — pick a named style (Dialogue, Thought…)
/// for the selected text, or set the new-text defaults from one; Styles…
/// opens the manager where editing a style reflows the chapter.
pub(crate) fn sec_text_workstyle(ui: &mut egui::Ui, app: &mut App) {
    let target = crate::text_edit::property_target(app);
    let current: Option<String> = target.and_then(|(li, ti)| {
        app.doc
            .layers
            .get(li)?
            .texts()?
            .texts
            .get(ti)?
            .style
            .clone()
    });
    ui.horizontal(|ui| {
        let shown = current
            .clone()
            .or_else(|| app.text_style_new.clone())
            .unwrap_or_else(|| "(none)".into());
        egui::ComboBox::from_id_salt("mn.text.workstyle")
            .width(118.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                let names: Vec<String> =
                    app.doc.text_styles.iter().map(|s| s.name.clone()).collect();
                for n in names {
                    let picked = current.as_deref() == Some(n.as_str());
                    if ui.selectable_label(picked, &n).clicked() && !picked {
                        // Assign to the selected text AND become the
                        // new-text default — both halves live in the one
                        // command, which the Ctrl+K palette runs too.
                        app.push_cmd(crate::cmd::AppCmd::TextStylePick(n.clone()));
                    }
                }
                ui.separator();
                if ui.selectable_label(false, "(none)").clicked() {
                    if let Some((li, ti)) = target
                        && current.is_some()
                    {
                        app.push_cmd(crate::cmd::AppCmd::TextStyleAssign {
                            layer: li,
                            item: ti,
                            name: None,
                        });
                    }
                    app.text_style_new = None;
                }
            })
            .response
            .on_hover_text(
                "the work style this text follows — edit the style and every \
                 balloon carrying it reflows, chapter-wide",
            );
        if ui.button("Styles…").clicked() {
            app.text_styles_open = true;
        }
    });
}

pub(crate) fn sec_text_guide(ui: &mut egui::Ui, app: &mut App) {
    ui.weak(if app.text_editing() {
        "typing… Esc commits (one undo step)"
    } else if app.tool == Tool::Object {
        "drag the body to move; corners/edges resize; the knob rotates"
    } else {
        "click the page to type; drag to set a wrap box; O moves/rotates"
    });
}
