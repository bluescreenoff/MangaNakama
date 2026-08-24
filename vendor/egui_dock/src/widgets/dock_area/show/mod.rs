use duplicate::duplicate;
use egui::{
    Context, CornerRadius, CursorIcon, EventFilter, Id, Key, Pos2, Rect, Sense, StrokeKind, Ui,
    Vec2,
};
use paste::paste;

use super::{drag_and_drop::TreeComponent, state::State, tab_removal::TabRemoval};
use crate::NodePath;
use crate::dock_area::tab_removal::ForcedRemoval;
use crate::tab_viewer::OnCloseResponse;
use crate::{
    AllowedSplits, DockArea, Node, NodeIndex, OverlayType, Split, Style, SurfaceIndex,
    TabDestination, TabInsert,
    TabViewer,
    utils::{expand_to_pixel, fade_dock_style, map_to_pixel},
};

mod leaf;
mod main_surface;
mod window_surface;

// MN-PATCH (MangaNakama, round 22): the drag/hover DnD payload keys are
// GLOBAL instead of per-DockArea-id. The app shows TWO sibling DockAreas
// (left/right palette columns around the canvas), each with its own
// DockState and its own `id`; with per-area keys a tab dragged in one area
// is invisible to the other, so cross-column drops can never resolve and a
// release over the canvas (over no leaf at all) silently snaps back. With
// global keys the leaf under the pointer — in EITHER area — publishes the
// hover payload every frame, and whichever DockArea OWNS the dragged tab
// (see the surface-bounds guard below) consumes the pair on release.
pub(super) fn dnd_drag_key() -> Id {
    Id::new("egui_dock::mn_dnd::drag_data")
}
pub(super) fn dnd_hover_key() -> Id {
    Id::new("egui_dock::mn_dnd::hover_data")
}

impl<Tab> DockArea<'_, Tab> {
    /// Shows the docking hierarchy inside a [`Ui`].
    pub fn show_inside(mut self, ui: &mut Ui, tab_viewer: &mut impl TabViewer<Tab = Tab>) {
        self.style
            .get_or_insert(Style::from_egui(ui.style().as_ref()));
        self.window_bounds.get_or_insert(ui.ctx().content_rect());
        // MN-PATCH #18: the dock area's own rect, for the root-edge drop
        // zones below (window_bounds covers the whole window incl. menu and
        // status bars — the edge strips must not extend into those).
        let dock_rect = ui.available_rect_before_wrap();

        let mut state = State::load(ui.ctx(), self.id);

        // Delay hover position one frame. On touch screens hover_pos() is None when any_released()
        if !ui.input(|i| i.pointer.any_released()) {
            state.last_hover_pos = ui.input(|i| i.pointer.hover_pos());
        }

        let (drag_data, hover_any) = ui.memory_mut(|mem| {
            (
                // MN-PATCH: global keys (see dnd_drag_key).
                mem.data.remove_temp(dnd_drag_key()).flatten(),
                mem.data.remove_temp(dnd_hover_key()).flatten(),
            )
        });
        // MN-PATCH: only hovers published by THIS DockArea's tree may be
        // indexed here — foreign node paths belong to the sibling DockState
        // and would panic.
        let hover_data = hover_any
            .clone()
            .filter(|h: &super::drag_and_drop::HoverData| h.owner == self.id);

        // MN-PATCH: only the DockArea that started the drag may process it
        // (DragData carries the owner id — see drag_and_drop.rs). With
        // global keys both sibling areas see every drag; the foreign one
        // would otherwise move tabs it does not have.
        let owns_drag = drag_data
            .as_ref()
            .is_some_and(|d: &super::drag_and_drop::DragData| d.owner == self.id);

        match (drag_data, owns_drag) {
            (Some(source), true) => {
                // MN-PATCH #18 (MangaNakama, 2026-08-22): root-edge drop
                // zones. A strip along the dock area's left/right edge means
                // "make the dragged tab a brand-new OUTERMOST column on that
                // side" (owner ask: drag a palette to the far left of the
                // screen → a new column left of everything, tools included).
                // Takes precedence over leaf hovers and float-window moves —
                // at the very edge the gesture is unambiguous.
                let edge = state
                    .last_hover_pos
                    .and_then(|p| super::drag_and_drop::edge_split_zone(dock_rect, p));
                let edge = match (edge, &source.src) {
                    (Some((split, zone)), &TreeComponent::Tab(src)) => Some((split, zone, src)),
                    _ => None,
                };
                if let Some((split, zone, src)) = edge {
                    super::drag_and_drop::draw_edge_zone(ui, self.style.as_ref().unwrap(), zone);
                    if ui.input(|i| i.pointer.primary_released()) {
                        // The new column's share of the window; the helper
                        // translates it through the by-position fraction rule.
                        const EDGE_SPLIT_FRACTION: f32 = 0.2;
                        self.dock_state
                            .move_tab_to_root_split(src, split, EDGE_SPLIT_FRACTION);
                    }
                }
                // MN-PATCH #19 (MangaNakama, 2026-08-26): the
                // BETWEEN-COLUMNS drop. Hovering a leaf's interior edge
                // strip means "insert as a new column right here" — the
                // owner's col1-between-col2-and-col3 ask, which the binary
                // tree expresses as a leaf-edge split. Checked AFTER the
                // root edges (they own the rim) and BEFORE the leaf hover
                // (at the edge, "insert beside this pane" is the truer
                // reading than the tab-bar overlay's arrows).
                else if let (Some((path, split, zone)), &TreeComponent::Tab(src)) = (
                    state
                        .last_hover_pos
                        .and_then(|p| self.leaf_edge_hover(dock_rect, p)),
                    &source.src,
                ) {
                    super::drag_and_drop::draw_edge_zone(ui, self.style.as_ref().unwrap(), zone);
                    if ui.input(|i| i.pointer.primary_released()) {
                        const LEAF_SPLIT_FRACTION: f32 = 0.3;
                        self.dock_state
                            .move_tab_to_leaf_edge_split(src, path, split, LEAF_SPLIT_FRACTION);
                    }
                }
                // MN-PATCH #14 (MangaNakama, 2026-08-21): a tab drag that
                // STARTED in a floating window surface moves the WINDOW,
                // unless the pointer is over a drop target that means
                // something (see `float_drag_moves_window`).
                else if self.float_drag_moves_window(&source, hover_data.as_ref()) {
                    self.drag_move_window(ui, &mut state, source.src.surface_address());
                } else if let Some(hover) = hover_data {
                    let style = self.style.as_ref().unwrap();
                    state.set_drag_and_drop(source, hover, ui.ctx(), style);
                    let tab_dst = self.show_drag_drop_overlay(ui, &mut state, tab_viewer);
                    // MN-PATCH #16: the viewer may refuse a tab JOINING an
                    // existing leaf's tab bar (`can_tab_into`) — canvas panes
                    // and palettes never mix. Split destinations pass; only
                    // the insert-as-tab ones are vetoed. A vetoed tab that
                    // is allowed in windows FLOATS at the pointer instead
                    // (dropping a palette over the canvas pane must keep
                    // behaving like patch #3's tear-off); one that is not
                    // (the canvas pane) drops to nothing and snaps back.
                    // The veto sits here at the commit, where the source
                    // and destination leaves can both be read immutably;
                    // the hover overlay still highlights the vetoed tab
                    // bar (cosmetic, accepted).
                    let tab_dst = tab_dst.and_then(|dst| {
                        let dst_node = match &dst {
                            TabDestination::Node(
                                path,
                                TabInsert::Insert(_) | TabInsert::Append,
                            ) => *path,
                            _ => return Some(dst),
                        };
                        let dnd = state.dnd.as_ref().unwrap();
                        let pointer = dnd.pointer;
                        let src_rect = dnd.drag.rect;
                        let TreeComponent::Tab(src_path) = dnd.drag.src else {
                            return Some(dst);
                        };
                        let vetoed = {
                            let src_tab = self.dock_state[src_path.node_path()]
                                .get_leaf()
                                .and_then(|l| l.tabs.get(src_path.tab.0));
                            let dst_leaf = self.dock_state[dst_node].get_leaf();
                            match (src_tab, dst_leaf) {
                                (Some(t), Some(l)) => !tab_viewer.can_tab_into(t, &l.tabs),
                                _ => false,
                            }
                        };
                        if !vetoed {
                            return Some(dst);
                        }
                        let floatable = self.dock_state[src_path.node_path()]
                            .get_leaf_mut()
                            .and_then(|l| l.tabs.get_mut(src_path.tab.0))
                            .is_some_and(|t| tab_viewer.allowed_in_windows(t));
                        floatable.then(|| {
                            TabDestination::Window(Rect::from_min_size(
                                pointer,
                                src_rect.size(),
                            ))
                        })
                    });
                    if ui.input(|i| i.pointer.primary_released())
                        && let Some(destination) = tab_dst
                    {
                        let source = {
                            match state.dnd.as_ref().unwrap().drag.src {
                                TreeComponent::Tab(src) => src,
                                _ => todo!(
                                    "collections of tabs, like nodes and surfaces can't be docked (yet)"
                                ),
                            }
                        };
                        let src_tab_count = self.dock_state[source.node_path()].tabs_count();
                        self.dock_state.move_tab(source, destination);
                        // unhide tab bar on the source leaf when a tab is dragged out
                        if self.hidable_tab_bars
                            && let Ok(leaf) = self.dock_state.leaf_mut(source.node_path())
                            && leaf.tabs.len() < src_tab_count
                            && leaf.tab_bar_hidden
                        {
                            leaf.tab_bar_hidden = false;
                        }
                    }
                } else if ui.input(|i| i.pointer.primary_released()) {
                    // MN-PATCH: released over NO dock leaf (the canvas, the gap
                    // between columns, anywhere): tear the tab off into a
                    // floating window at the pointer instead of snapping back.
                    // Upstream only ever drops onto a leaf, which made
                    // "drag out to float" impossible outside the dock itself.
                    let TreeComponent::Tab(src) = source.src else {
                        todo!(
                            "collections of tabs, like nodes and surfaces can't be docked (yet)"
                        )
                    };
                    // MN-PATCH #16 addition: a tab barred from windows (the
                    // canvas pane) never tears off — the drop simply snaps
                    // back instead of producing a floating window the
                    // viewer said may not exist.
                    let floatable = self.dock_state[src.node_path()]
                        .get_leaf_mut()
                        .and_then(|l| l.tabs.get_mut(src.tab.0))
                        .is_some_and(|t| tab_viewer.allowed_in_windows(t));
                    if floatable {
                        let rect = Rect::from_min_size(
                            state.last_hover_pos.unwrap_or(Pos2::ZERO),
                            source.rect.size(),
                        );
                        self.dock_state.move_tab(src, TabDestination::Window(rect));
                    }
                }
            }
            // MN-PATCH: a foreign drag (owned by the sibling DockArea): put
            // BOTH payloads back. This area's read must not eat the hover
            // published by the owner's own leaves this frame — the owner
            // consumes the pair at the top of ITS next pass.
            (Some(source), false) => {
                ui.memory_mut(|mem| {
                    mem.data.insert_temp(dnd_drag_key(), Some(source));
                    if let Some(hover) = hover_any {
                        mem.data.insert_temp(dnd_hover_key(), Some(hover));
                    }
                });
            }
            _ => {}
        }

        if ui.input(|i| i.pointer.primary_released()) {
            state.reset_drag();
        }

        let style = self.style.as_ref().unwrap();
        let fade_surface =
            self.hovered_window_surface(&mut state, style.overlay.feel.fade_hold_time, ui.ctx());
        let fade_style = {
            fade_surface.is_some().then(|| {
                let mut fade_style = style.clone();
                fade_dock_style(&mut fade_style, style.overlay.surface_fade_opacity);
                (fade_style, style.overlay.surface_fade_opacity)
            })
        };

        for &surface_index in self.dock_state.valid_surface_indices().iter() {
            self.show_surface_inside(
                surface_index,
                ui,
                tab_viewer,
                &mut state,
                fade_style.as_ref().map(|(style, factor)| {
                    (style, *factor, fade_surface.unwrap_or(SurfaceIndex::main()))
                }),
            );
        }

        for removal in self.to_remove.drain(..).rev() {
            match removal {
                TabRemoval::Tab(path, ForcedRemoval(is_forced)) => {
                    if is_forced {
                        self.dock_state.remove_tab(path);
                    } else {
                        let leaf = &mut self.dock_state.leaf_mut(path.node_path()).unwrap();
                        match tab_viewer.on_close(&mut leaf.tabs[path.tab.0]) {
                            OnCloseResponse::Close => {
                                self.dock_state.remove_tab(path);
                            }
                            OnCloseResponse::Focus => {
                                leaf.active = path.tab;
                                self.new_focused = Some(path.node_path());
                            }
                            OnCloseResponse::Ignore => {
                                // no-op
                            }
                        }
                    }
                }
                TabRemoval::Node(path) => {
                    let mut all_tabs_are_closable = true;
                    for tab in self.dock_state[path].iter_tabs_mut() {
                        if !(tab_viewer.is_closeable(tab)
                            && matches!(tab_viewer.on_close(tab), OnCloseResponse::Close))
                        {
                            all_tabs_are_closable = false;
                        }
                    }
                    if all_tabs_are_closable {
                        self.dock_state.remove_leaf(path);
                    }
                }
                TabRemoval::Window(surface) => {
                    let mut all_tabs_are_closable = true;
                    for node in self.dock_state[surface].iter_mut() {
                        for tab in node.iter_tabs_mut() {
                            if !(tab_viewer.is_closeable(tab)
                                && matches!(tab_viewer.on_close(tab), OnCloseResponse::Close))
                            {
                                all_tabs_are_closable = false;
                            }
                        }
                    }
                    if all_tabs_are_closable {
                        self.dock_state.remove_surface(surface);
                    }
                }
            }
        }

        for path in self.to_detach.drain(..).rev() {
            let mouse_pos = state.last_hover_pos;
            self.dock_state.detach_tab(
                path,
                Rect::from_min_size(
                    mouse_pos.unwrap_or(Pos2::ZERO),
                    self.dock_state[path.node_path()]
                        .rect()
                        .map_or(Vec2::new(100., 150.), |rect| rect.size()),
                ),
            );
        }

        if let Some(focused) = self.new_focused {
            self.dock_state.set_focused_node_and_surface(focused);
        }

        state.store(ui.ctx(), self.id);
    }

    /// MN-PATCH #14 (MangaNakama, 2026-08-21): should this in-flight tab drag
    /// MOVE its floating window instead of dragging the tab out of it?
    ///
    /// The app floats palettes as `Surface::Window` entries with `title_bar`
    /// off and `fill_tab_bar` on, so the tab strip IS the window's title bar —
    /// grabbing it has to feel like grabbing a title bar. Before this patch
    /// every such drag ended in MN-PATCH #3's no-hover fallback
    /// (`TabDestination::Window`), which runs `detach_tab`: a BRAND NEW
    /// surface at the pointer with a reset position and size, while the old
    /// one was collected — the owner screenshotted the result (2026-08-21).
    ///
    /// Drops that still mean something keep their old behaviour, so this
    /// returns `false` for them:
    /// * the drag started on the main surface (an ordinary dock drag / the
    ///   tear-off this patch does not touch),
    /// * the pointer is over a leaf of ANOTHER surface — re-docking a float
    ///   into a column, or into a different float,
    /// * the pointer is over a DIFFERENT node of the same window (an
    ///   intra-window re-dock),
    /// * the pointer is on its own leaf's tab strip and that leaf has more
    ///   than one tab — a reorder.
    fn float_drag_moves_window(
        &self,
        drag: &super::drag_and_drop::DragData,
        hover: Option<&super::drag_and_drop::HoverData>,
    ) -> bool {
        let TreeComponent::Tab(src) = drag.src else {
            return false;
        };
        if src.surface.is_main() {
            return false;
        }
        let Some(hover) = hover else {
            // Over the canvas, another column, nothing: pure window move.
            return true;
        };
        let (dst_surface, dst_node) = hover.dst.node_address();
        if dst_surface != src.surface {
            return false;
        }
        match dst_node {
            Some(node) if node != src.node => false,
            _ => !(hover.tab.is_some() && self.dock_state[src.node_path()].tabs_count() > 1),
        }
    }

    /// MN-PATCH #14: drive the EXISTING window surface by the pointer delta.
    ///
    /// The position is re-read from egui every frame rather than accumulated,
    /// because `WindowState::create_window` constrains the window to
    /// `window_bounds`: a fast drag past the edge is clamped, and an
    /// accumulator would keep running past the clamp and leave the window
    /// stuck until the pointer came all the way back.
    fn drag_move_window(&mut self, ui: &Ui, state: &mut State, surface: SurfaceIndex) {
        state.float_move = true;
        // No drop overlay, no window fade: this drag is not going anywhere.
        state.dnd = None;
        state.window_fade = None;
        let delta = ui.input(|i| i.pointer.delta());
        if delta == Vec2::ZERO {
            return;
        }
        let id = window_surface::window_area_id(surface);
        let Some(rect) = ui.ctx().memory(|mem| mem.area_rect(id)) else {
            return;
        };
        if let Some(window_state) = self.dock_state.get_window_state_mut(surface) {
            window_state.set_position(rect.min + delta);
        }
    }

    /// Returns some when windows are fading, and what surface index is being hovered over
    #[inline(always)]
    fn hovered_window_surface(
        &self,
        state: &mut State,
        hold_time: f32,
        ctx: &Context,
    ) -> Option<SurfaceIndex> {
        if let Some(dnd_state) = &state.dnd
            && dnd_state.is_locked(self.style.as_ref().unwrap(), ctx)
        {
            state.window_fade =
                Some((ctx.input(|i| i.time), dnd_state.hover.dst.surface_address()));
        }

        state.window_fade.and_then(|(time, surface)| {
            ctx.request_repaint();
            (hold_time > (ctx.input(|i| i.time) - time) as f32).then_some(surface)
        })
    }

    /// Resolve where a dragged tab would land given it's dropped this frame, returns `None` when the resulting drop is an invalid move.
    fn show_drag_drop_overlay(
        &mut self,
        ui: &Ui,
        state: &mut State,
        tab_viewer: &impl TabViewer<Tab = Tab>,
    ) -> Option<TabDestination> {
        let drag_state = state.dnd.as_mut().unwrap();
        let style = self.style.as_ref().unwrap();

        let deserted_node = {
            match (
                drag_state.drag.src.node_address(),
                drag_state.hover.dst.node_address(),
            ) {
                ((src_surf, Some(src_node)), (dst_surf, Some(dst_node))) => {
                    src_surf == dst_surf
                        && src_node == dst_node
                        && self.dock_state[src_surf][src_node].tabs_count() == 1
                }
                _ => false,
            }
        };

        // Not all scenarios can house all splits.
        let restricted_splits = if drag_state.hover.dst.is_surface() || deserted_node {
            AllowedSplits::None
        } else {
            AllowedSplits::All
        };
        let allowed_splits = self.allowed_splits & restricted_splits;

        let allowed_in_window = match drag_state.drag.src {
            TreeComponent::Tab(path) => {
                let Node::Leaf(leaf) = &mut self.dock_state[path.node_path()] else {
                    unreachable!("tab drags can only come from leaf nodes")
                };
                tab_viewer.allowed_in_windows(&mut leaf.tabs[path.tab.0])
            }
            _ => todo!("collections of tabs, like nodes or surfaces, can't be dragged! (yet)"),
        };

        if let Some(pointer) = state.last_hover_pos {
            drag_state.pointer = pointer;
        }

        let window_bounds = self.window_bounds.unwrap();
        match (style.overlay.overlay_type, drag_state.is_on_title_bar()) {
            (OverlayType::HighlightedAreas, _) | (_, true) => drag_state.resolve_traditional(
                ui,
                style,
                allowed_splits,
                allowed_in_window,
                window_bounds,
            ),
            (OverlayType::Widgets, false) => drag_state.resolve_icon_based(
                ui,
                style,
                allowed_splits,
                allowed_in_window,
                window_bounds,
            ),
        }
    }

    /// Show a single surface of a [`DockState`].
    fn show_surface_inside(
        &mut self,
        surf_index: SurfaceIndex,
        ui: &mut Ui,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        state: &mut State,
        fade_style: Option<(&Style, f32, SurfaceIndex)>,
    ) {
        if surf_index.is_main() {
            self.show_root_surface_inside(ui, tab_viewer, state);
        } else {
            self.show_window_surface(ui, surf_index, tab_viewer, state, fade_style);
        }
    }

    fn render_nodes(
        &mut self,
        ui: &mut Ui,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        state: &mut State,
        surf_index: SurfaceIndex,
        fade_style: Option<(&Style, f32)>,
    ) {
        // First compute all rect sizes in the node graph.
        let max_rect = self.allocate_area_for_root_node(ui, surf_index);
        for node_index in self.dock_state[surf_index].breadth_first_index_iter() {
            let path = NodePath {
                surface: surf_index,
                node: node_index,
            };
            if self.dock_state[path].is_parent() {
                self.compute_rect_sizes(ui, path, max_rect);
            }
        }

        // Then, draw the bodies of each leaves.
        for node_index in self.dock_state[surf_index].breadth_first_index_iter() {
            let path = NodePath {
                surface: surf_index,
                node: node_index,
            };
            if self.dock_state[path].is_leaf() {
                self.show_leaf(ui, state, path, tab_viewer, fade_style);
            }
        }

        // Finally, draw separators so that their "interaction zone" is above
        // bodies (see `SeparatorStyle::extra_interact_width`).
        let fade_style = fade_style.map(|(style, _)| style);
        for node_index in self.dock_state[surf_index].breadth_first_index_iter() {
            let path = NodePath {
                surface: surf_index,
                node: node_index,
            };
            if self.dock_state[surf_index][node_index].is_parent() {
                self.show_separator(ui, path, fade_style);
            }
        }
    }

    fn allocate_area_for_root_node(&mut self, ui: &mut Ui, surface: SurfaceIndex) -> Rect {
        let style = self.style.as_ref().unwrap();
        let full_rect = ui.available_rect_before_wrap();
        let mut rect = full_rect;

        if let Some(margin) = style.dock_area_padding {
            rect.min += margin.left_top();
            rect.max -= margin.right_bottom();
        }

        ui.painter().rect_stroke(
            rect,
            style.main_surface_border_rounding,
            style.main_surface_border_stroke,
            StrokeKind::Inside,
        );
        if surface == SurfaceIndex::main() {
            rect = rect.expand(-style.main_surface_border_stroke.width / 2.0);
        }
        // MN-PATCH (MangaNakama, 2026-08-16): allocate the FULL available rect,
        // not the dock-area-padded one. A resizable host `egui::Panel` stores
        // its content-sized rect every frame (the `Frame::show` response), so
        // a dock whose reported min size is narrower than the allocation
        // ratchets the panel closed by the padding on every frame — the owner
        // watched a dragged-wider palette column shrink back 1pt per repaint.
        // Claiming the full rect makes the panel's stored size a fixed point.
        // The tree still lays out inside the padded `rect` below.
        ui.allocate_rect(full_rect, Sense::hover());

        if self.dock_state[surface].is_empty() {
            return rect;
        }
        self.dock_state[surface][NodeIndex::root()].set_rect(rect);
        rect
    }

    /// MN-PATCH #19: the leaf whose interior edge strip holds `pointer`,
    /// as (that leaf's path, the split side, the strip rect). Leaves whose
    /// edge IS the dock area's rim are skipped inside
    /// [`leaf_edge_zone`](super::drag_and_drop::leaf_edge_zone) — #18 owns
    /// the rim. Main surface only: the between-columns gesture is a
    /// column-layout thing, and windows float alone.
    fn leaf_edge_hover(
        &self,
        dock_rect: egui::Rect,
        pointer: egui::Pos2,
    ) -> Option<(NodePath, Split, egui::Rect)> {
        let surface = SurfaceIndex::main();
        for (index, node) in self.dock_state[surface].iter().enumerate() {
            let Node::Leaf(leaf) = node else {
                continue;
            };
            if let Some((split, zone)) =
                super::drag_and_drop::leaf_edge_zone(leaf.rect(), dock_rect, pointer)
            {
                let path = NodePath {
                    surface,
                    node: NodeIndex(index),
                };
                return Some((path, split, zone));
            }
        }
        None
    }

    fn compute_rect_sizes(&mut self, ui: &Ui, path: NodePath, max_rect: Rect) {
        assert!(self.dock_state[path].is_parent());

        let style = self.style.as_ref().unwrap();
        let pixels_per_point = ui.ctx().pixels_per_point();

        let left_collapsed_count = self.dock_state[path.left_node()].collapsed_leaf_count();
        let right_collapsed_count = self.dock_state[path.right_node()].collapsed_leaf_count();
        let left_collapsed = self.dock_state[path.left_node()].is_collapsed();
        let right_collapsed = self.dock_state[path.right_node()].is_collapsed();

        if (left_collapsed || right_collapsed)
            && let Node::Vertical(split) = &mut self.dock_state[path.surface][path.node]
        {
            let rect = split.rect();
            debug_assert!(!rect.any_nan() && rect.is_finite());
            let rect = expand_to_pixel(rect, pixels_per_point);

            if left_collapsed {
                // EITHER only left collapsed OR left and right both collapsed
                let border_y = rect.min.y + (left_collapsed_count as f32) * style.tab_bar.height;
                let left_separator_border = map_to_pixel(
                    border_y - style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round,
                );
                let right_separator_border = map_to_pixel(
                    border_y + style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round,
                );
                let left = rect
                    .intersect(Rect::everything_above(left_separator_border))
                    .intersect(max_rect);
                let right = rect
                    .intersect(Rect::everything_below(right_separator_border))
                    .intersect(max_rect);
                self.dock_state[path.left_node()].set_rect(left);
                self.dock_state[path.right_node()].set_rect(right);
            } else {
                // Only right collapsed
                let border_y = rect.max.y - (right_collapsed_count as f32) * style.tab_bar.height;
                let left_separator_border = map_to_pixel(
                    border_y - style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round,
                );
                let right_separator_border = map_to_pixel(
                    border_y + style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round,
                );
                let left = rect
                    .intersect(Rect::everything_above(left_separator_border))
                    .intersect(max_rect);
                let right = rect
                    .intersect(Rect::everything_below(right_separator_border))
                    .intersect(max_rect);
                self.dock_state[path.left_node()].set_rect(left);
                self.dock_state[path.right_node()].set_rect(right);
            }
            return;
        }

        duplicate! {
            [
                orientation   dim_point  dim_size  left_of    right_of;
                [Horizontal]  [x]        [width]   [left_of]  [right_of];
                [Vertical]    [y]        [height]  [above]    [below];
            ]
            if let Node::orientation(split) = &mut self.dock_state[path.surface][path.node] {
                let rect = split.rect;
                debug_assert!(!rect.any_nan() && rect.is_finite());
                let rect = expand_to_pixel(rect, pixels_per_point);

                let dim_size = rect.dim_size();
                let midpoint = if dim_size > 0.0 {
                    rect.min.dim_point + dim_size * split.fraction
                } else {
                    rect.min.dim_point
                };

                let left_separator_border = map_to_pixel(
                    midpoint - style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round
                );
                let right_separator_border = map_to_pixel(
                    midpoint + style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round
                );

                paste! {
                    let left = rect.intersect(Rect::[<everything_ left_of>](left_separator_border)).intersect(max_rect);
                    let right = rect.intersect(Rect::[<everything_ right_of>](right_separator_border)).intersect(max_rect);
                }

                self.dock_state[path.left_node()].set_rect(left);
                self.dock_state[path.right_node()].set_rect(right);
            }
        }
    }

    fn show_separator(&mut self, ui: &mut Ui, path: NodePath, fade_style: Option<&Style>) {
        assert!(self.dock_state[path.surface][path.node].is_parent());

        // If either of the children is collapsed, we don't want the user to interact with the separator
        if (self.dock_state[path.left_node()].is_collapsed()
            || self.dock_state[path.right_node()].is_collapsed())
            && self.dock_state[path.surface][path.node].is_vertical()
        {
            return;
        }

        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let pixels_per_point = ui.ctx().pixels_per_point();

        duplicate! {
            [
                orientation   dim_point  dim_size;
                [Horizontal]  [x]        [width];
                [Vertical]    [y]        [height];
            ]
            if let Node::orientation(split) = &mut self.dock_state[path.surface][path.node] {
                let rect = split.rect;
                let mut separator = rect;

                let midpoint = rect.min.dim_point + rect.dim_size() * split.fraction;
                separator.min.dim_point = midpoint - style.separator.width * 0.5;
                separator.max.dim_point = midpoint + style.separator.width * 0.5;

                let mut expand = Vec2::ZERO;
                expand.dim_point += style.separator.extra_interact_width / 2.0;
                let interact_rect = separator.expand2(expand);

                let response = ui.allocate_rect(interact_rect, Sense::click_and_drag())
                    .on_hover_and_drag_cursor(paste!{ CursorIcon::[<Resize orientation>]});

                let should_respond_to_arrow_keys = ui.input(|i| i.modifiers.command || i.modifiers.shift);

                if response.has_focus() {
                    // Prevent the default behaviour of removing focus from the separators when the
                    // arrow keys are pressed
                    ui.memory_mut(|m| m.set_focus_lock_filter(response.id, EventFilter {
                        horizontal_arrows: should_respond_to_arrow_keys,
                        vertical_arrows: should_respond_to_arrow_keys,
                        tab: false,
                        escape: false
                    }));
                }

                let arrow_key_offset = if response.has_focus() && should_respond_to_arrow_keys {
                    if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
                        Some(egui::vec2(0., -16.))
                    } else if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
                        Some(egui::vec2(0., 16.))
                    } else if ui.input(|i| i.key_pressed(Key::ArrowLeft)) {
                        Some(egui::vec2(-16., 0.))
                    } else if ui.input(|i| i.key_pressed(Key::ArrowRight)) {
                        Some(egui::vec2(16., 0.))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let midpoint = rect.min.dim_point + rect.dim_size() * split.fraction;
                separator.min.dim_point = map_to_pixel(
                    midpoint - style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round,
                );
                separator.max.dim_point = map_to_pixel(
                    midpoint + style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round,
                );

                let color = if response.dragged() {
                    style.separator.color_dragged
                } else if response.hovered() || response.has_focus() {
                    style.separator.color_hovered
                } else {
                    style.separator.color_idle
                };

                ui.painter().rect_filled(separator, CornerRadius::ZERO, color);

                // Update 'fraction' interaction after drawing separator,
                // otherwise it may overlap on other separator / bodies when
                // shrunk fast.
                let range = rect.max.dim_point - rect.min.dim_point;
                if range > 0.0 {
                    let min = (style.separator.extra / range).min(1.0);
                    let max = 1.0 - min;
                    let (min, max) = (min.min(max), max.max(min));
                    let delta = arrow_key_offset.unwrap_or(response.drag_delta()).dim_point;
                    split.fraction = (split.fraction + delta / range).clamp(min, max);
                }

                if response.double_clicked() {
                    split.fraction = 0.5;
                }
            }
        }
    }
}
