//! What a dropped file means (IO-041 / IO-042 — the gesture every new user
//! tries in the first minute, and which `grep -rn DroppedFile crates/` proved
//! we did not have at all).
//!
//! # Our fork, and why it is deliberate
//!
//! CSP splits this gesture by WHERE you let go: drop on the canvas and it
//! builds a NEW DOCUMENT out of the image; drop on the Layer palette and it
//! imports into the current one. We do not split it. Wherever you let go, an
//! image lands as a layer in the document you are already drawing — because
//! that is the drop a mangaka actually makes (a reference photo onto the page
//! in progress), and because the other meaning already has its own gesture:
//! **drop a project (`.mnc`/`.ora`, or a work folder) and it opens.**
//!
//! A gesture whose meaning turns on a few pixels of cursor position is a
//! gesture you have to aim; this one you do not.
//!
//! # The one rule that matters here
//!
//! A drop that does nothing and says nothing is the worst outcome — the user
//! cannot tell "unsupported" from "broken". So: when the plan is empty, it
//! carries a note explaining why, and the caller shows it. When the plan is
//! NOT empty, there is no note, because each command reports its own result
//! and a note would only be overwritten.

use std::path::{Path, PathBuf};

use crate::cmd::AppCmd;

/// What we can actually decode. This list must not outrun the `image` crate
/// features in the workspace `Cargo.toml` — offering a format we cannot read
/// turns a clear "cannot import" into a baffling "import failed".
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp", "gif"];

fn ext_lower(p: &Path) -> Option<String> {
    p.extension().map(|e| e.to_string_lossy().to_lowercase())
}

fn is_image(p: &Path) -> bool {
    ext_lower(p).is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()))
}

/// A project the drop should OPEN, resolved to the path `OpenOraPath` wants.
/// A dropped work FOLDER resolves to its `work.mnc` index — dragging the
/// folder is the natural gesture for a format whose whole point is that a
/// comic is a folder.
fn as_project(p: &Path) -> Option<PathBuf> {
    if p.is_dir() {
        let index = p.join(mn_core::project::WORKFOLDER_INDEX);
        return index.is_file().then_some(index);
    }
    match ext_lower(p).as_deref() {
        Some("mnc") | Some("ora") => Some(p.to_path_buf()),
        _ => None,
    }
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Turn a set of dropped paths into commands, plus a note to show when the
/// result is "nothing happened".
///
/// A project anywhere in the drop wins over images: dropping a chapter and a
/// photo together means "open the chapter" far more often than it means
/// "import the photo into whatever is open now", and opening is the
/// destructive-ish one to get wrong.
pub fn plan(paths: &[PathBuf]) -> (Vec<AppCmd>, Option<String>) {
    if paths.is_empty() {
        return (Vec::new(), None);
    }

    if let Some(project) = paths.iter().find_map(|p| as_project(p)) {
        return (vec![AppCmd::OpenOraPath(project)], None);
    }

    let images: Vec<&PathBuf> = paths.iter().filter(|p| is_image(p)).collect();
    if images.is_empty() {
        let what = if paths.len() == 1 {
            name_of(&paths[0])
        } else {
            format!("{} files", paths.len())
        };
        return (
            Vec::new(),
            Some(format!(
                "cannot open {what} — drop an image ({}), or a .mnc / .ora project",
                IMAGE_EXTS.join(", ")
            )),
        );
    }

    let cmds = images
        .into_iter()
        .map(|p| AppCmd::ImportImagePath(p.clone()))
        .collect();
    (cmds, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn is_import(c: &AppCmd, want: &str) -> bool {
        matches!(c, AppCmd::ImportImagePath(q) if q == &p(want))
    }

    #[test]
    fn one_image_imports_as_a_layer() {
        let (cmds, note) = plan(&[p("C:/ref/pose.png")]);
        assert_eq!(cmds.len(), 1);
        assert!(is_import(&cmds[0], "C:/ref/pose.png"));
        assert!(note.is_none(), "a plan that acts must not also nag");
    }

    #[test]
    fn several_images_import_in_drop_order() {
        let (cmds, _) = plan(&[p("a/one.png"), p("a/two.JPG"), p("a/three.webp")]);
        assert_eq!(cmds.len(), 3);
        assert!(is_import(&cmds[0], "a/one.png"));
        assert!(is_import(&cmds[1], "a/two.JPG"), "extension match is case-insensitive");
        assert!(is_import(&cmds[2], "a/three.webp"));
    }

    #[test]
    fn a_project_opens_and_wins_over_images_in_the_same_drop() {
        let (cmds, note) = plan(&[p("ref/photo.png"), p("work/ch1.ora")]);
        assert_eq!(cmds.len(), 1, "opening a document is not something to do twice");
        assert!(matches!(&cmds[0], AppCmd::OpenOraPath(q) if q == &p("work/ch1.ora")));
        assert!(note.is_none());
    }

    #[test]
    fn unsupported_files_produce_no_command_but_do_explain() {
        let (cmds, note) = plan(&[p("notes/script.txt")]);
        assert!(cmds.is_empty());
        let note = note.expect("a drop that does nothing must say why");
        assert!(note.contains("script.txt"), "name the file the user dropped: {note}");
        assert!(note.contains("png"), "say what we DO take: {note}");
    }

    #[test]
    fn many_unsupported_files_are_counted_not_listed() {
        let (_, note) = plan(&[p("a.txt"), p("b.psd"), p("c.doc")]);
        assert!(note.unwrap().contains("3 files"));
    }

    #[test]
    fn an_empty_drop_is_silent() {
        let (cmds, note) = plan(&[]);
        assert!(cmds.is_empty());
        assert!(note.is_none(), "nothing dropped is not an error to report");
    }

    /// A dropped work FOLDER resolves to its index file — the gesture for a
    /// format whose unit is a directory.
    #[test]
    fn a_work_folder_resolves_to_its_index() {
        let dir = std::env::temp_dir().join(format!("mn-drop-{}", std::process::id()));
        let index = dir.join(mn_core::project::WORKFOLDER_INDEX);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&index, b"not a real index, only its name matters here").unwrap();

        let (cmds, note) = plan(&[dir.clone()]);
        assert!(matches!(&cmds[0], AppCmd::OpenOraPath(q) if q == &index));
        assert!(note.is_none());

        // A plain folder is not a project and must not silently do nothing
        // without saying so.
        let plain = dir.join("empty");
        std::fs::create_dir_all(&plain).unwrap();
        let (cmds, note) = plan(&[plain]);
        assert!(cmds.is_empty());
        assert!(note.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }
}
