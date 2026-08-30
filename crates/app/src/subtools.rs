//! Sub tool GROUPS as data, and the shortcut TARGETING model built on them
//! (owner ask 2026-08-25).
//!
//! `ui/subtool.rs` is the palette that DRAWS this; `cmd::SubTool` is the row
//! enumeration it draws. What was missing between them was the middle layer:
//! the group captions were string literals typed into the drawing code, so
//! nothing outside that function could point at a tab. Here they are constants
//! ([`group`]), the tool → groups → rows tree is a registry derived from
//! `SubTool::ALL` ([`groups_of`]), and a shortcut can name any level of it:
//!
//! * [`Target::SubTool`] — always that exact row.
//! * [`Target::Group`] — that tab; the row comes from the tab's own memory.
//! * [`Target::Tool`] — the tool; the tab comes from the tool's memory, then
//!   the row from that tab's.
//!
//! Two or more targets on one key = press again to cycle, in bound order —
//! which is what CSP does when several tools share a letter (the owner's `U`
//! is Figure → Frame Border → Ruler). The cycle is STATELESS: the next target
//! is the one after whichever target currently matches the app, so a palette
//! click or a `,`/`.` step between presses never leaves a stale index behind.
//!
//! MEMORY. Within a session the app's own mode fields ARE the memory — every
//! tool already remembers its mode, which is why pressing a tool key must not
//! reset it. So [`is_current`] reads that live state and the ui.txt map only
//! has to carry it ACROSS sessions: [`note_memory`] snapshots it beside every
//! ui.txt save, [`restore_from_memory`] plays it back once at startup.
//!
//! Not registry-backed yet, deliberately: the Figure line PRESET rows
//! (Stream line / Dense stream / …) are parameter sets on one sub tool rather
//! than sub tools of their own — a tweaked preset highlights no row at all,
//! which is exactly what makes them presets. `Target::Group(Figure, "Stream
//! line")` therefore lands on the stream sub tool carrying whatever preset is
//! loaded, and naming an individual preset row waits for the sub tool
//! registry to grow a parameter axis.

use crate::app::App;
use crate::cmd::{AppCmd, SubTool, Tool};

/// The Sub Tool list's group captions. The palette draws THESE (never a
/// literal), so a caption and the thing a shortcut can name cannot drift.
pub mod group {
    pub const FILL: &str = "Fill";
    pub const TONE: &str = "Tone";
    pub const AUTO_SELECT: &str = "Auto select";
    pub const SELECTION: &str = "Selection";
    pub const CREATE_FRAME: &str = "Create frame";
    pub const CUT_FRAME: &str = "Cut frame border";
    pub const BALLOON: &str = "Balloon";
    pub const TEXT: &str = "Text";
    pub const OPERATION: &str = "Operation";
    pub const DIRECT_DRAW: &str = "Direct draw";
    pub const STREAM_LINE: &str = "Stream line";
    pub const SATURATED_LINE: &str = "Saturated line";
    pub const GRADIENT: &str = "Gradient";
    pub const EYEDROPPER: &str = "Eyedropper";
    pub const AVERAGE_COLOR: &str = "Average color";
    pub const MOVE: &str = "Move";
}

/// One tab of a tool's Sub Tool list.
pub struct Group {
    pub name: &'static str,
    pub subs: Vec<SubTool>,
}

/// Which caption a row is drawn under. The tools with ONE group answer their
/// own name; only Frame border, Figure and the Eyedropper split.
pub fn group_of(s: SubTool) -> &'static str {
    use crate::cmd::{FigureMode as FM, FrameMode as FR};
    match s {
        SubTool::FillRefer(_) | SubTool::Fill(_) => group::FILL,
        SubTool::Tone(_) => group::TONE,
        SubTool::Wand(_) => group::AUTO_SELECT,
        SubTool::Select(_) | SubTool::SelectPen | SubTool::SelectEraser => group::SELECTION,
        SubTool::Frame(FR::Rect | FR::Polyline | FR::Pen) => group::CREATE_FRAME,
        SubTool::Frame(FR::DivideFolder | FR::DivideBorder) => group::CUT_FRAME,
        SubTool::Balloon(_) => group::BALLOON,
        SubTool::Text => group::TEXT,
        SubTool::Object(_) => group::OPERATION,
        SubTool::Figure(
            FM::Line | FM::Rect | FM::Ellipse | FM::Polygon | FM::Arc | FM::Curve | FM::Smart,
        ) => group::DIRECT_DRAW,
        SubTool::Figure(FM::Stream) => group::STREAM_LINE,
        SubTool::Figure(FM::Focus | FM::Urchin | FM::SolidFlash) => group::SATURATED_LINE,
        SubTool::Gradient(_) => group::GRADIENT,
        SubTool::Eyedrop(_) => group::EYEDROPPER,
        SubTool::EyedropSize(_) => group::AVERAGE_COLOR,
        SubTool::Pan(_) => group::MOVE,
    }
}

/// The tool whose Sub Tool LIST a row is listed in. Differs from
/// `SubTool::tool` for exactly the selection pen pair: those carry their own
/// `Tool` (the canvas stroke paths key off it) but are listed, and targeted,
/// under Selection.
pub fn owner(tool: Tool) -> Tool {
    match tool {
        Tool::SelPen | Tool::SelEraser => Tool::Select,
        t => t,
    }
}

type Registry = Vec<(Tool, Vec<Group>)>;

/// tool → its groups, in `SubTool::ALL`'s order — which is the order the
/// palette draws. Built once; `&'static` so a caller can hold it across a
/// `&mut App` (the palette does exactly that while drawing).
fn registry() -> &'static Registry {
    static REG: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    REG.get_or_init(|| {
        let mut out: Registry = Vec::new();
        for &s in SubTool::ALL {
            let tool = owner(s.tool());
            let name = group_of(s);
            if !out.iter().any(|(t, _)| *t == tool) {
                out.push((tool, Vec::new()));
            }
            let groups = &mut out
                .iter_mut()
                .find(|(t, _)| *t == tool)
                .expect("just inserted")
                .1;
            match groups.last_mut() {
                Some(g) if g.name == name => g.subs.push(s),
                _ => groups.push(Group {
                    name,
                    subs: vec![s],
                }),
            }
        }
        out
    })
}

/// A tool's sub tool groups. Empty for the two INK tools (their list is the
/// brush presets, which are files, not an enumeration) and for Liquify
/// (its modes live in Tool Property, one flat radio list).
pub fn groups_of(tool: Tool) -> &'static [Group] {
    let tool = owner(tool);
    registry()
        .iter()
        .find(|(t, _)| *t == tool)
        .map(|(_, g)| g.as_slice())
        .unwrap_or(&[])
}

/// One group's rows, in list order — what the palette draws a tab from.
/// An unknown tab is empty, never a panic.
pub fn rows(tool: Tool, group: &str) -> &'static [SubTool] {
    groups_of(tool)
        .iter()
        .find(|g| g.name == group)
        .map(|g| g.subs.as_slice())
        .unwrap_or(&[])
}

/// Whether this row is the one its tool is currently SET to — mode only, and
/// deliberately blind to which tool is in hand: `select_mode` still says
/// "Lasso" while you are drawing with the pen, and that is the memory a `M`
/// press has to honour.
pub fn is_current(app: &App, s: SubTool) -> bool {
    use crate::cmd::FillMode;
    match s {
        SubTool::FillRefer(r) => app.fill_mode == FillMode::Click && app.fill_opts.refer == r,
        SubTool::Fill(m) => app.fill_mode == m,
        SubTool::Tone(p) => app.tone_opts.tone.pattern == p,
        SubTool::Wand(r) => app.wand_opts.refer == r,
        SubTool::Select(m) => app.select_mode == m,
        // The create-type IS the tool for these two.
        SubTool::SelectPen => app.tool == Tool::SelPen,
        SubTool::SelectEraser => app.tool == Tool::SelEraser,
        SubTool::Frame(m) => app.frame_mode == m,
        SubTool::Balloon(m) => app.balloon_mode == m,
        SubTool::Text => true,
        SubTool::Object(m) => app.object_mode == m,
        SubTool::Figure(m) => app.figure_mode == m,
        SubTool::Gradient(m) => app.grad_mode == m,
        SubTool::Eyedrop(r) => app.eyedrop_opts.refer == r,
        SubTool::EyedropSize(n) => app.eyedrop_opts.size == n,
        SubTool::Pan(m) => app.pan_mode == m,
    }
}

/// What the palette HIGHLIGHTS: the current row of the tool in hand. The
/// selection shapes carry the extra gate the list has always had — holding
/// the selection pen must not also light "Rectangle", or the list stops
/// saying where you are.
pub fn is_lit(app: &App, s: SubTool) -> bool {
    if let SubTool::Select(_) = s {
        return app.tool == Tool::Select && is_current(app, s);
    }
    owner(app.tool) == owner(s.tool()) && is_current(app, s)
}

/// The group a tool is currently in — the tab a bare `Target::Tool` lands on.
pub fn current_group(app: &App, tool: Tool) -> Option<&'static str> {
    groups_of(tool)
        .iter()
        .find(|g| g.subs.iter().any(|&s| is_current(app, s)))
        .map(|g| g.name)
}

/// Put one sub tool's state into the app WITHOUT switching tools — the half
/// of `AppCmd::SetSubTool` that the startup restore reuses. The two modes
/// that own commands go through those commands, so a palette click, a
/// palette-search pick and a shortcut all run the same code, status lines and
/// mid-gesture cleanups included.
pub fn apply_state(app: &mut App, s: SubTool) {
    use crate::cmd::{FillMode, dispatch};
    match s {
        SubTool::FillRefer(r) => {
            app.fill_opts.refer = r;
            dispatch(app, AppCmd::SetFillMode(FillMode::Click));
        }
        SubTool::Fill(m) => dispatch(app, AppCmd::SetFillMode(m)),
        SubTool::Tone(p) => app.tone_opts.tone.pattern = p,
        SubTool::Wand(r) => app.wand_opts.refer = r,
        SubTool::Select(m) => dispatch(app, AppCmd::SetSelectMode(m)),
        // The create-type IS the tool, and the `SetTool` in the command's
        // own arm already switched to it.
        SubTool::SelectPen | SubTool::SelectEraser => {}
        SubTool::Frame(m) => {
            app.frame_mode = m;
            app.frame_poly = None;
            app.frame_pen = None;
        }
        SubTool::Balloon(m) => app.balloon_mode = m,
        SubTool::Text => {}
        SubTool::Object(m) => app.object_mode = m,
        SubTool::Figure(m) => {
            app.figure_mode = m;
            app.figure_poly = None;
            app.figure_stage2 = None;
            app.smart_shape = None;
        }
        SubTool::Gradient(m) => {
            app.grad_mode = m;
            // FI-050: a half-drawn freeform gesture belongs to the row that
            // started it — leaving it armed would let the second guide line
            // land in a mode that has no use for it.
            app.grad_free = None;
        }
        SubTool::Eyedrop(r) => app.eyedrop_opts.refer = r,
        SubTool::EyedropSize(n) => app.eyedrop_opts.size = n,
        SubTool::Pan(m) => app.pan_mode = m,
    }
}

// --- targets ------------------------------------------------------------

/// A row's address in the registry: tool ▸ tab ▸ row. `tool` is the LIST's
/// tool (`owner`), so the selection pen's path is Select ▸ Selection ▸
/// Selection pen even though pressing it leaves you on `Tool::SelPen`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubToolPath {
    pub tool: Tool,
    pub group: &'static str,
    pub sub: SubTool,
}

impl SubToolPath {
    pub fn of(sub: SubTool) -> Self {
        Self {
            tool: owner(sub.tool()),
            group: group_of(sub),
            sub,
        }
    }
}

/// What a key press aims at. Three kinds, one cycle rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The tool; the tab from its memory, then the row from that tab's.
    Tool(Tool),
    /// That tab; the row from the tab's memory.
    Group(Tool, &'static str),
    /// Exactly that row, always.
    SubTool(SubToolPath),
}

impl Target {
    pub fn tool(&self) -> Tool {
        match self {
            Target::Tool(t) => *t,
            Target::Group(t, _) => *t,
            Target::SubTool(p) => p.tool,
        }
    }

    /// Is the app AT this target right now? The cycle rule's only state.
    pub fn matches(&self, app: &App) -> bool {
        if owner(app.tool) != owner(self.tool()) {
            return false;
        }
        match self {
            Target::Tool(_) => true,
            Target::Group(t, g) => current_group(app, *t) == Some(*g),
            Target::SubTool(p) => is_current(app, p.sub),
        }
    }

    /// The row this target resolves to, memory and all. `None` = the tool has
    /// no sub tool rows (Pen, Eraser, Liquify) and the press is a plain tool
    /// switch.
    pub fn resolve(&self, app: &App) -> Option<SubTool> {
        if let Target::SubTool(p) = self {
            return Some(p.sub);
        }
        let tool = self.tool();
        let groups = groups_of(tool);
        let want = match self {
            Target::Group(_, g) => Some(*g),
            _ => remembered_group(app, tool),
        };
        let g = want
            .and_then(|w| groups.iter().find(|g| g.name == w))
            .or_else(|| groups.first())?;
        // Live state first: it IS this session's memory, and it is the only
        // reading that survives a `,`/`.` step or a Tool Property edit.
        g.subs
            .iter()
            .copied()
            .find(|&s| is_current(app, s))
            .or_else(|| remembered_sub(app, tool, g.name))
            .or_else(|| g.subs.first().copied())
    }
}

/// One press of a key bound to `targets`: pick the next one in bound order
/// and queue it. ONE command, and a tool-bearing one, so `main::key_down`'s
/// tail check arms the spring for free.
pub fn press(app: &mut App, targets: &[Target]) {
    let Some(first) = targets.first() else { return };
    let at = targets.iter().position(|t| t.matches(app));
    let next = at.map_or(0, |i| (i + 1) % targets.len());
    let target = targets.get(next).unwrap_or(first);
    match target.resolve(app) {
        // The tool is ALREADY set to that row, so only the tool in hand can
        // still change: a plain `SetTool`, which is exactly what every tool
        // key queued before the targeting model existed. Re-applying instead
        // would be a behaviour change dressed as a no-op — `SetSubTool` runs
        // the row's own arm, and those arms write status lines and drop
        // in-progress gestures.
        Some(s) if is_current(app, s) => app.push_cmd(AppCmd::SetTool(s.tool())),
        Some(s) => app.push_cmd(AppCmd::SetSubTool(s)),
        None => app.push_cmd(AppCmd::SetTool(target.tool())),
    }
}

// --- ui.txt memory ------------------------------------------------------

fn group_key(tool: Tool) -> String {
    tool.label().to_owned()
}

fn sub_key(tool: Tool, group: &str) -> String {
    format!("{}/{group}", tool.label())
}

fn remembered_group(app: &App, tool: Tool) -> Option<&'static str> {
    // The live tab wins, for the same reason the live row does.
    if let Some(g) = current_group(app, tool) {
        return Some(g);
    }
    let want = app.layout.sub_tool_last.get(&group_key(tool))?;
    groups_of(tool)
        .iter()
        .find(|g| g.name == want)
        .map(|g| g.name)
}

fn remembered_sub(app: &App, tool: Tool, group: &str) -> Option<SubTool> {
    let want = app.layout.sub_tool_last.get(&sub_key(tool, group))?;
    groups_of(tool)
        .iter()
        .find(|g| g.name == group)?
        .subs
        .iter()
        .copied()
        .find(|s| s.label() == want)
}

/// Snapshot the live per-tool / per-group picks into `ui.txt`'s map. Called
/// beside every layout save rather than on every switch, so the file cannot
/// disagree with the app: the app's own mode fields are the source, whatever
/// moved them.
pub fn note_memory(app: &mut App) {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (tool, groups) in registry() {
        for g in groups {
            if let Some(s) = g.subs.iter().copied().find(|&s| is_current(app, s)) {
                pairs.push((sub_key(*tool, g.name), s.label().to_owned()));
            }
        }
        if let Some(g) = current_group(app, *tool) {
            pairs.push((group_key(*tool), g.to_owned()));
        }
    }
    app.layout.note_sub_tool_last(pairs);
}

/// Play the map back once at startup — the half that makes it MEMORY rather
/// than a log. Modes only; it never picks the tool the app boots on.
pub fn restore_from_memory(app: &mut App) {
    if app.layout.sub_tool_last.is_empty() {
        return;
    }
    // `apply_state` routes two rows through commands that set a status line;
    // at startup that would leave "fill: click an area" on the bar as the
    // app's first word to the user.
    let status = std::mem::take(&mut app.status);
    for (tool, groups) in registry() {
        let Some(gname) = app.layout.sub_tool_last.get(&group_key(*tool)).cloned() else {
            continue;
        };
        let Some(g) = groups.iter().find(|g| g.name == gname) else {
            continue;
        };
        let Some(sname) = app.layout.sub_tool_last.get(&sub_key(*tool, g.name)).cloned() else {
            continue;
        };
        let Some(&s) = g.subs.iter().find(|s| s.label() == sname) else {
            continue;
        };
        // Rows that ARE a tool (the selection pen pair) have no state of
        // their own — `apply_state` does nothing for them, which is what
        // keeps this from deciding the tool the app boots on.
        apply_state(app, s);
    }
    app.status = status;
}

// --- keys.json target specs ---------------------------------------------

/// The `keys.json` spelling of a target: `tool: Figure`, `tool: Figure /
/// Stream line`, `tool: Figure / Stream line / Stream line`. Names are the
/// palette's own (`Tool::label`, the group caption, the row label), matched
/// case-insensitively; the error is what the user reads in the status bar.
pub fn parse_target(spec: &str) -> Result<Target, String> {
    let body = spec
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(spec)
        .trim();
    let parts: Vec<&str> = body.split('/').map(str::trim).collect();
    let tool = tool_named(parts[0]).ok_or_else(|| format!("no tool called \"{}\"", parts[0]))?;
    match parts[..] {
        [_] => Ok(Target::Tool(tool)),
        [_, g] => {
            let g = group_named(tool, g)
                .ok_or_else(|| format!("{} has no sub tool group \"{g}\"", tool.label()))?;
            Ok(Target::Group(tool, g))
        }
        [_, g, s] => {
            let gname = group_named(tool, g)
                .ok_or_else(|| format!("{} has no sub tool group \"{g}\"", tool.label()))?;
            let sub = groups_of(tool)
                .iter()
                .find(|gr| gr.name == gname)
                .and_then(|gr| {
                    gr.subs
                        .iter()
                        .copied()
                        .find(|x| x.label().eq_ignore_ascii_case(s))
                })
                .ok_or_else(|| format!("\"{gname}\" has no sub tool \"{s}\""))?;
            // `of` re-derives the same tool and group from the row, which is
            // the canonical spelling: "Select / Selection / Selection pen"
            // and the row itself must produce the same path.
            Ok(Target::SubTool(SubToolPath::of(sub)))
        }
        _ => Err(format!(
            "\"{body}\" — a target is tool, tool / group, or tool / group / sub tool"
        )),
    }
}

fn tool_named(name: &str) -> Option<Tool> {
    registry()
        .iter()
        .map(|(t, _)| *t)
        // The ink tools have no rows, so they are not in the registry — a
        // shortcut can still name them.
        .chain([Tool::Pen, Tool::Eraser, Tool::Liquify])
        .find(|t| t.label().eq_ignore_ascii_case(name))
}

fn group_named(tool: Tool, name: &str) -> Option<&'static str> {
    groups_of(tool)
        .iter()
        .find(|g| g.name.eq_ignore_ascii_case(name))
        .map(|g| g.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::UiLayout;
    use crate::cmd::{BalloonMode, FigureMode, FrameMode, PanMode, SelectMode, dispatch};

    fn headless() -> Option<App> {
        let mut app = App::new(crate::app::headless_renderer()?, (600, 400), 1.0);
        // App::new reads the machine's own ui.txt (the test exe shares one
        // with every other test); the memory tests own this map.
        app.layout = UiLayout::default();
        Some(app)
    }

    /// The registry is the palette's own tree: every row in `SubTool::ALL`
    /// appears exactly once, under its own caption, in list order.
    #[test]
    fn the_registry_holds_every_row_once() {
        let mut seen = 0usize;
        for (tool, groups) in registry() {
            for g in groups {
                for &s in &g.subs {
                    assert_eq!(group_of(s), g.name, "{:?} filed under {}", s, g.name);
                    assert_eq!(owner(s.tool()), *tool);
                    seen += 1;
                }
            }
        }
        assert_eq!(seen, SubTool::ALL.len(), "no row lost, none doubled");
        // The three splits are the point of having groups at all.
        let names = |t: Tool| -> Vec<&'static str> {
            groups_of(t).iter().map(|g| g.name).collect()
        };
        assert_eq!(
            names(Tool::Frame),
            vec![group::CREATE_FRAME, group::CUT_FRAME]
        );
        assert_eq!(
            names(Tool::Figure),
            vec![group::DIRECT_DRAW, group::STREAM_LINE, group::SATURATED_LINE]
        );
        assert_eq!(
            names(Tool::Eyedrop),
            vec![group::EYEDROPPER, group::AVERAGE_COLOR]
        );
        // The selection pen pair is listed under Selection, not on its own.
        assert_eq!(names(Tool::Select), vec![group::SELECTION]);
        assert_eq!(
            groups_of(Tool::SelPen).as_ptr(),
            groups_of(Tool::Select).as_ptr(),
            "the selection pen is listed under Selection, not on its own"
        );
        assert!(groups_of(Tool::Pen).is_empty(), "brush presets are files");
        assert!(groups_of(Tool::Liquify).is_empty(), "modes are properties");
    }

    /// The three target kinds, each resolving through one more level of
    /// memory than the last.
    #[test]
    fn the_three_target_kinds_resolve() {
        let Some(mut app) = headless() else { return };
        // 1. SubTool — the exact row, whatever the app is doing.
        let exact = Target::SubTool(SubToolPath::of(SubTool::Frame(FrameMode::Pen)));
        assert_eq!(exact.resolve(&app), Some(SubTool::Frame(FrameMode::Pen)));

        // 2. Group — the tab's own last row. Live state is that memory:
        // put the tool on Divide frame border and the CUT tab answers it
        // while the CREATE tab still answers its own.
        app.frame_mode = FrameMode::DivideBorder;
        let cut = Target::Group(Tool::Frame, group::CUT_FRAME);
        assert_eq!(
            cut.resolve(&app),
            Some(SubTool::Frame(FrameMode::DivideBorder))
        );
        let create = Target::Group(Tool::Frame, group::CREATE_FRAME);
        assert_eq!(
            create.resolve(&app),
            Some(SubTool::Frame(FrameMode::Rect)),
            "no memory for the tab yet — its first row"
        );

        // 3. Tool — the tool's last TAB, then that tab's last row.
        assert_eq!(
            Target::Tool(Tool::Frame).resolve(&app),
            Some(SubTool::Frame(FrameMode::DivideBorder)),
            "the tool remembers it was in the Cut tab"
        );
        app.figure_mode = FigureMode::Focus;
        assert_eq!(
            current_group(&app, Tool::Figure),
            Some(group::SATURATED_LINE)
        );
        assert_eq!(
            Target::Tool(Tool::Figure).resolve(&app),
            Some(SubTool::Figure(FigureMode::Focus))
        );
        // A tool with no rows resolves to nothing — a plain tool switch.
        assert_eq!(Target::Tool(Tool::Pen).resolve(&app), None);
    }

    /// Two targets on one key = repeat-press cycles, and the cycle reads the
    /// app rather than a stored index: a tool change from anywhere else
    /// re-aims it.
    #[test]
    fn a_shared_key_cycles_in_bound_order() {
        let Some(mut app) = headless() else { return };
        let both = [Target::Tool(Tool::Text), Target::Tool(Tool::Balloon)];
        let pump = |app: &mut App, keys: &[Target]| {
            press(app, keys);
            while let Some(c) = app.cmds.pop_front() {
                dispatch(app, c);
            }
            app.tool
        };
        assert_eq!(pump(&mut app, &both), Tool::Text, "from elsewhere: first");
        assert_eq!(pump(&mut app, &both), Tool::Balloon, "again: next");
        assert_eq!(pump(&mut app, &both), Tool::Text, "and round");
        // Somebody else moves the tool; the cycle re-aims from where it is.
        dispatch(&mut app, AppCmd::SetTool(Tool::Balloon));
        assert_eq!(pump(&mut app, &both), Tool::Text);
    }

    /// T and E used to be hand-written flips in `main.rs`. The table version
    /// must be the same machine, including "from any third tool, first".
    #[test]
    fn the_migrated_t_and_e_cycles_behave_as_before() {
        let Some(mut app) = headless() else { return };
        let t = [Target::Tool(Tool::Text), Target::Tool(Tool::Balloon)];
        let e = [Target::Tool(Tool::Eraser), Target::Tool(Tool::Pen)];
        let run = |app: &mut App, keys: &[Target]| {
            press(app, keys);
            while let Some(c) = app.cmds.pop_front() {
                dispatch(app, c);
            }
            app.tool
        };
        // The old bodies, verbatim in intent.
        let old_t = |tool: Tool| {
            if tool == Tool::Text {
                Tool::Balloon
            } else {
                Tool::Text
            }
        };
        let old_e = |tool: Tool| {
            if tool == Tool::Eraser {
                Tool::Pen
            } else {
                Tool::Eraser
            }
        };
        for start in [Tool::Pen, Tool::Eraser, Tool::Text, Tool::Balloon, Tool::Fill] {
            dispatch(&mut app, AppCmd::SetTool(start));
            assert_eq!(run(&mut app, &t), old_t(start), "T from {start:?}");
            dispatch(&mut app, AppCmd::SetTool(start));
            assert_eq!(run(&mut app, &e), old_e(start), "E from {start:?}");
        }
    }

    /// A tool key whose row the tool is ALREADY on queues the plain tool
    /// switch it always did — never a sub tool re-apply, whose arm would
    /// write a status line and drop in-progress gestures the old key left
    /// alone. The sub tool half runs only when the row actually changes.
    #[test]
    fn a_tool_key_only_re_applies_a_row_that_changed() {
        let Some(mut app) = headless() else { return };
        app.frame_mode = FrameMode::Polyline;
        for from in [Tool::Frame, Tool::Pen] {
            dispatch(&mut app, AppCmd::SetTool(from));
            app.cmds.clear();
            press(&mut app, &[Target::Tool(Tool::Frame)]);
            assert!(
                matches!(app.cmds.back(), Some(AppCmd::SetTool(Tool::Frame))),
                "from {from:?}: a plain tool set, got {:?}",
                app.cmds.back()
            );
        }
        // A row that differs is the case the sub tool command exists for.
        app.cmds.clear();
        press(
            &mut app,
            &[Target::SubTool(SubToolPath::of(SubTool::Frame(
                FrameMode::DivideBorder,
            )))],
        );
        assert!(
            matches!(
                app.cmds.back(),
                Some(AppCmd::SetSubTool(SubTool::Frame(FrameMode::DivideBorder)))
            ),
            "{:?}",
            app.cmds.back()
        );
    }

    /// The snapshot half, and the owner's actual sentence: "each group
    /// remembers its own last sub tool". A tool has ONE mode field, so the
    /// tab you are not in can only be remembered by the map — which is why
    /// the snapshot MERGES instead of replacing.
    /// (`app::tests::the_sub_tool_memory_round_trips_through_ui_txt` drives
    /// the same map through a real ui.txt write and read.)
    #[test]
    fn each_group_remembers_its_own_row() {
        let Some(mut app) = headless() else { return };
        let got = |app: &App, k: &str| app.layout.sub_tool_last.get(k).cloned();

        // Work in the Create frame tab...
        app.frame_mode = FrameMode::Pen;
        note_memory(&mut app);
        assert_eq!(got(&app, "Frame border").as_deref(), Some(group::CREATE_FRAME));
        assert_eq!(
            got(&app, "Frame border/Create frame").as_deref(),
            Some("Frame border pen")
        );

        // ...then move to the Cut tab. The tool's tab moves with you; the
        // tab you left keeps the row you left it on.
        app.frame_mode = FrameMode::DivideFolder;
        note_memory(&mut app);
        assert_eq!(got(&app, "Frame border").as_deref(), Some(group::CUT_FRAME));
        assert_eq!(
            got(&app, "Frame border/Cut frame border").as_deref(),
            Some("Divide frame folder")
        );
        assert_eq!(
            got(&app, "Frame border/Create frame").as_deref(),
            Some("Frame border pen"),
            "the tab you left keeps its own row"
        );
        // And that is what a group-aimed key lands on — not the tab's first
        // row, the row you left there.
        assert_eq!(
            Target::Group(Tool::Frame, group::CREATE_FRAME).resolve(&app),
            Some(SubTool::Frame(FrameMode::Pen))
        );

        app.balloon_mode = BalloonMode::Tail;
        app.figure_mode = FigureMode::Urchin;
        note_memory(&mut app);
        assert_eq!(got(&app, "Figure").as_deref(), Some(group::SATURATED_LINE));
        assert_eq!(
            got(&app, "Balloon/Balloon").as_deref(),
            Some("Balloon tail"),
            "rows are stored under the label the palette shows"
        );
    }

    /// The playback half, and its failure mode: names this build cannot place
    /// (a newer ui.txt, a hand edit) are skipped one by one — the rest still
    /// restore, and nothing panics.
    #[test]
    fn the_memory_restores_and_skips_what_it_cannot_place() {
        let Some(mut app) = headless() else { return };
        for (k, v) in [
            ("Frame border", "Nonexistent tab"),
            ("Balloon", "Balloon"),
            ("Balloon/Balloon", "Balloon tail"),
            ("Figure", "Direct draw"),
            ("Figure/Direct draw", "Hyperbola"),
            ("Move view", "Move"),
            ("Move view/Move", "Rotate"),
            ("Select", "Selection"),
            ("Select/Selection", "Selection pen"),
        ] {
            app.layout
                .sub_tool_last
                .insert(k.to_owned(), v.to_owned());
        }
        app.balloon_mode = BalloonMode::Ellipse;
        app.figure_mode = FigureMode::Line;
        app.frame_mode = FrameMode::Rect;
        app.select_mode = SelectMode::Rect;
        app.pan_mode = PanMode::Hand;
        let tool = app.tool;
        restore_from_memory(&mut app);
        assert_eq!(app.balloon_mode, BalloonMode::Tail, "the good one restored");
        assert_eq!(app.pan_mode, PanMode::Rotate, "and this one");
        assert_eq!(app.figure_mode, FigureMode::Line, "unknown row skipped");
        assert_eq!(app.frame_mode, FrameMode::Rect, "unknown tab skipped");
        assert_eq!(app.select_mode, SelectMode::Rect, "a row that IS a tool");
        assert_eq!(app.tool, tool, "the restore never picks the boot tool");
        assert!(app.status.is_empty(), "and never speaks over the status bar");
    }

    /// The keys.json spelling of each target kind, and what a typo says.
    #[test]
    fn target_specs_parse_at_every_level() {
        assert_eq!(parse_target("tool: Pen"), Ok(Target::Tool(Tool::Pen)));
        assert_eq!(
            parse_target("tool: frame border / cut frame border"),
            Ok(Target::Group(Tool::Frame, group::CUT_FRAME))
        );
        assert_eq!(
            parse_target("tool: Figure / Direct draw / Ellipse"),
            Ok(Target::SubTool(SubToolPath::of(SubTool::Figure(
                FigureMode::Ellipse
            ))))
        );
        // The selection pen is reached under Selection, where it is listed.
        assert_eq!(
            parse_target("tool: Select / Selection / Selection pen"),
            Ok(Target::SubTool(SubToolPath::of(SubTool::SelectPen)))
        );
        for bad in [
            "tool: Nope",
            "tool: Figure / Nope",
            "tool: Figure / Direct draw / Nope",
            "tool: a / b / c / d",
        ] {
            assert!(parse_target(bad).is_err(), "{bad}");
        }
    }
}
