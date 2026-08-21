//! Named workspaces (TRIAGE 146 v1, UI-060/061/063/064): snapshots of the
//! palette layout (both dock columns as JSON, the column widths, the
//! Tool Property section visibility) under a name. UI-062's
//! import-checkboxes (shortcuts/Command Bar/units) defer until those
//! exist as separate configs — a workspace today is the LAYOUT only,
//! which is what never travels anyway. UI-065's reset stays the existing
//! "Reset layout".

use super::App;

/// Field indices inside a workspace entry. Shipped API — an index never
/// changes meaning; new fields only ever go on the END, where an older
/// build's shorter entry simply lacks them.
const WS_NAME: usize = 0;
const WS_DOCK_LEFT: usize = 1;
const WS_DOCK_RIGHT: usize = 2;
const WS_LEFT_W: usize = 3;
const WS_RIGHT_W: usize = 4;
const WS_PROP_HIDDEN: usize = 5;
// 6 and 7 were the column collapse flags — meaningless since docking 2
// (the single tree has no column collapse), never reused.
/// Docking 2: the whole tree as one JSON field. A workspace registered by
/// an older build lacks it and migrates from fields 1..=4 at APPLY time.
const WS_DOCK_TREE: usize = 8;

impl App {
    /// One field of a workspace entry, or `""` when the entry is an older
    /// build's and simply does not carry it. Every read goes through here:
    /// entries used to be a fixed six fields, and a bare `e[6]` would panic
    /// on the first workspace the user registered before this build.
    fn ws_field(entry: &[String], index: usize) -> &str {
        entry.get(index).map_or("", String::as_str)
    }

    fn persist(&mut self) {
        let ws = serde_json::to_string(&self.workspaces).unwrap_or_default();
        self.layout
            .note_workspaces(&ws, &self.workspace_current.clone());
    }

    /// UI-060: snapshot the live layout under a name (re-registering an
    /// existing name overwrites it).
    pub fn workspace_register(&mut self, name: &str) {
        let entry = vec![
            name.to_string(),
            // Fields 1..=4 stay EMPTY on a docking-2 register: the tree in
            // field 8 is the layout. An older build applying this entry
            // falls back to its default columns — graceful, not wrong.
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            self.layout.prop_hidden.clone(),
            String::new(),
            String::new(),
            crate::ui::dock::to_json_tree(&self.dock),
        ];
        if let Some(e) = self
            .workspaces
            .iter_mut()
            .find(|e| Self::ws_field(e, WS_NAME) == name)
        {
            *e = entry;
        } else {
            self.workspaces.push(entry);
        }
        self.workspace_current = name.to_string();
        self.persist();
    }

    /// UI-061: switch (or UI-063: reload) — apply a workspace by name.
    /// Returns false when the name does not exist.
    pub fn workspace_apply(&mut self, name: &str) -> bool {
        let Some(e) = self
            .workspaces
            .iter()
            .find(|e| Self::ws_field(e, WS_NAME) == name)
            .cloned()
        else {
            return false;
        };
        let f = |i| Self::ws_field(&e, i).to_string();
        let tree = f(WS_DOCK_TREE);
        self.dock = if !tree.is_empty() {
            crate::ui::dock::from_json_tree(&tree)
        } else {
            // A pre-docking-2 workspace: fold its two columns around a
            // canvas pane, exactly like the ui.txt migration (widths from
            // fields 3/4 against a nominal window — fractions, so close
            // enough; a drag away from exact).
            let left = if f(WS_DOCK_LEFT).is_empty() {
                crate::ui::dock::to_json(&crate::ui::dock::default_left())
            } else {
                f(WS_DOCK_LEFT)
            };
            let right = if f(WS_DOCK_RIGHT).is_empty() {
                crate::ui::dock::to_json(&crate::ui::dock::default_right())
            } else {
                f(WS_DOCK_RIGHT)
            };
            let lw = f(WS_LEFT_W).parse().unwrap_or(186.0);
            let rw = f(WS_RIGHT_W).parse().unwrap_or(208.0);
            crate::ui::dock::merge_columns(&left, &right, lw, rw, 1280.0)
                .unwrap_or_else(crate::ui::dock::default_tree)
        };
        self.layout.prop_hidden = f(WS_PROP_HIDDEN);
        self.workspace_current = name.to_string();
        self.persist();
        true
    }

    /// UI-063: snap the DRAGGED-AROUND layout back to the saved state.
    pub fn workspace_reload(&mut self) -> bool {
        let cur = self.workspace_current.clone();
        if cur.is_empty() {
            return false;
        }
        self.workspace_apply(&cur)
    }

    /// UI-064 (delete half): remove by name; the current may be removed.
    pub fn workspace_delete(&mut self, name: &str) {
        self.workspaces
            .retain(|e| Self::ws_field(e, WS_NAME) != name);
        if self.workspace_current == name {
            self.workspace_current.clear();
        }
        self.persist();
    }
}
