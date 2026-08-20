//! Named workspaces (TRIAGE 146 v1, UI-060/061/063/064): snapshots of the
//! palette layout (both dock columns as JSON, the column widths, the
//! Tool Property section visibility) under a name. UI-062's
//! import-checkboxes (shortcuts/Command Bar/units) defer until those
//! exist as separate configs — a workspace today is the LAYOUT only,
//! which is what never travels anyway. UI-065's reset stays the existing
//! "Reset layout".

use super::App;

impl App {
    fn persist(&mut self) {
        let ws = serde_json::to_string(&self.workspaces).unwrap_or_default();
        self.layout
            .note_workspaces(&ws, &self.workspace_current.clone());
    }

    /// UI-060: snapshot the live layout under a name (re-registering an
    /// existing name overwrites it).
    pub fn workspace_register(&mut self, name: &str) {
        let entry = [
            name.to_string(),
            crate::ui::dock::to_json(&self.dock_left),
            crate::ui::dock::to_json(&self.dock_right),
            format!("{:.0}", self.layout.left_w),
            format!("{:.0}", self.layout.right_w),
            self.layout.prop_hidden.clone(),
        ];
        if let Some(e) = self.workspaces.iter_mut().find(|e| e[0] == name) {
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
        let Some(e) = self.workspaces.iter().find(|e| e[0] == name).cloned() else {
            return false;
        };
        let left = crate::ui::dock::from_json(&e[1], crate::ui::dock::default_left);
        let right = crate::ui::dock::from_json(&e[2], crate::ui::dock::default_right);
        self.dock_left = left;
        self.dock_right = right;
        self.layout.left_w = e[3].parse().unwrap_or(self.layout.left_w);
        self.layout.right_w = e[4].parse().unwrap_or(self.layout.right_w);
        self.layout.prop_hidden = e[5].clone();
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
        self.workspaces.retain(|e| e[0] != name);
        if self.workspace_current == name {
            self.workspace_current.clear();
        }
        self.persist();
    }
}
