//! Clip Studio's `EditImageTool.todb` — the WHOLE tool database (T5b).
//!
//! A `.sut` is one exported slice of this same schema; that reader picks
//! the fatmost Variant row and hopes. Here the Node tree is walked
//! instead (`Manager.RootUuid` → children via `NodeFirstChildUuid`,
//! siblings via `NodeNextUuid` — the linked lists sutdump established):
//! every LEAF node is a sub tool, its `NodeVariantID` the live parameter
//! row, its ancestors the CSP group path. Each becomes the same
//! [`SutBrush`] the .sut rail produces, so the proven CSP→libmypaint
//! import (`sut_import::write_sut_import`) applies unchanged.
//!
//! Bitmap TIPS are not migrated on this path (v1, recorded): they ride
//! the `.sut` export. A bulk import brings every tool's DYNAMICS; export
//! a tip-bearing brush as `.sut` for its stamp.

use std::collections::BTreeMap;
use std::path::Path;

use crate::sqlite_ro::{self, Value};
use crate::sut::{SutBrush, variant_params_effectors};

/// One sub tool out of the database.
pub struct TobdTool {
    /// The sub tool's display name (`Node.NodeName`).
    pub name: String,
    /// The CSP group path above it (its Node ancestors, outermost first).
    pub group_path: Vec<String>,
    /// The parsed parameters — same shape a `.sut` of this tool yields.
    pub brush: SutBrush,
}

struct NodeRow {
    name: String,
    next: String,
    child: String,
    variant: i64,
}

/// Parse a tool database's bytes. Only the three tables the walk needs are
/// decoded (`parse_sqlite_tables`): a real database carries thousands of
/// tools and material blobs, and none of the rest is ours to read.
pub fn parse_todb(bytes: &[u8]) -> Result<Vec<TobdTool>, String> {
    let tables = sqlite_ro::parse_sqlite_tables(bytes, &["Node", "Manager", "Variant"])?;
    let nodes: BTreeMap<String, NodeRow> = tables
        .get("Node")
        .map(|t| {
            t.records()
                .into_iter()
                .filter_map(|r| {
                    let uuid = hex16(r.get("NodeUuid").and_then(Value::as_blob)?)?;
                    Some((
                        uuid,
                        NodeRow {
                            name: r
                                .get("NodeName")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .unwrap_or_default(),
                            next: r
                                .get("NodeNextUuid")
                                .and_then(Value::as_blob)
                                .and_then(hex16)
                                .unwrap_or_default(),
                            child: r
                                .get("NodeFirstChildUuid")
                                .and_then(Value::as_blob)
                                .and_then(hex16)
                                .unwrap_or_default(),
                            variant: r.get("NodeVariantID").and_then(Value::as_i64).unwrap_or(0),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    if nodes.is_empty() {
        return Err("todb: no Node table (not a tool database?)".into());
    }
    let variants: BTreeMap<i64, BTreeMap<String, Value>> = tables
        .get("Variant")
        .map(|t| {
            t.records()
                .into_iter()
                .filter_map(|r| {
                    let id = r.get("VariantID").and_then(Value::as_i64)?;
                    Some((id, r))
                })
                .collect()
        })
        .unwrap_or_default();

    let root = tables
        .get("Manager")
        .and_then(|t| t.records().into_iter().next())
        .and_then(|r| {
            r.get("RootUuid")
                .and_then(Value::as_blob)
                .map(|b| b.to_vec())
        })
        .and_then(|b| hex16(&b))
        .ok_or("todb: Manager has no RootUuid")?;

    // Depth-first from the root's children, group path carried on the
    // stack (sutdump's csp walk, minus the JSON): a node WITH children is
    // a group; a leaf with a live variant row is a sub tool.
    let mut out = Vec::new();
    let mut stack: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(r) = nodes.get(&root) {
        let mut ch = r.child.clone();
        let mut kids = Vec::new();
        while let Some(n) = nodes.get(&ch) {
            kids.push((ch.clone(), Vec::new()));
            ch = n.next.clone();
            if kids.len() > 500 {
                break;
            }
        }
        kids.reverse();
        stack = kids;
    }
    let mut hops = 0usize;
    while let Some((uuid, path)) = stack.pop() {
        hops += 1;
        if hops > 10_000 {
            return Err("todb: Node walk exceeded 10 000 tools (cycle?)".into());
        }
        let Some(n) = nodes.get(&uuid) else {
            continue;
        };
        let mut p = path.clone();
        p.push(n.name.clone());
        if !n.child.is_empty() {
            let mut ch = n.child.clone();
            let mut kids = Vec::new();
            while let Some(k) = nodes.get(&ch) {
                kids.push((ch.clone(), p.clone()));
                ch = k.next.clone();
                if kids.len() > 500 {
                    break;
                }
            }
            kids.reverse();
            stack.extend(kids);
            continue; // a group node, not a sub tool itself
        }
        if n.variant == 0 {
            continue;
        }
        let Some(rec) = variants.get(&n.variant) else {
            continue;
        };
        let (params, effectors) = variant_params_effectors(rec);
        if params.is_empty() {
            continue;
        }
        out.push(TobdTool {
            group_path: p[..p.len() - 1].to_vec(),
            name: n.name.clone(),
            brush: SutBrush {
                name: n.name.clone(),
                params,
                effectors,
                tip_pngs: Vec::new(),
            },
        });
    }
    Ok(out)
}

/// Parse a tool database from disk. WAL guard first: a LIVE Clip Studio
/// journals into an `EditImageTool.todb-wal` sidecar, and this reader
/// would silently see the stale main-file image — refuse loudly instead.
pub fn parse_todb_file(path: &Path) -> Result<Vec<TobdTool>, String> {
    if let Some(s) = path.to_str() {
        let wal = format!("{s}-wal");
        if Path::new(&wal).exists() {
            return Err(format!(
                "todb: {wal} exists — Clip Studio looks like it is running \
                 (its journal would be missed). Close it, or copy the .todb \
                 somewhere else first."
            ));
        }
    }
    let bytes = std::fs::read(path).map_err(|e| format!("todb: {}: {e}", path.display()))?;
    parse_todb(&bytes)
}

/// A CSP UUID blob is 16 bytes; the hex spelling is the map key.
fn hex16(b: &[u8]) -> Option<String> {
    (b.len() == 16).then(|| b.iter().map(|x| format!("{x:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Option<Vec<u8>> {
        let p =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/todb_sample.todb");
        std::fs::read(p).ok().or_else(|| {
            eprintln!("[fixture] todb_sample.todb missing, skipping");
            None
        })
    }

    /// T5b: the Node walk finds every LEAF sub tool, carries its CSP group
    /// path, and skips the non-tools (a childless group, a leaf with no
    /// live variant). The params are the same shape a .sut of the tool
    /// yields — sizes in the stored LENGTH unit, verbatim.
    #[test]
    fn the_walk_finds_leaves_with_groups_and_skips_non_tools() {
        let Some(b) = fixture() else { return };
        let tools = parse_todb(&b).expect("the fixture parses");
        let names: Vec<(&str, usize)> = tools
            .iter()
            .map(|t| (t.name.as_str(), t.group_path.len()))
            .collect();
        assert_eq!(
            names,
            vec![("Mapping pen", 1), ("G-pen", 1), ("Hard Airbrush", 1),],
            "three sub tools under Pen and Airbrush; the empty group and \
             the variantless leaf are not tools"
        );
        let map = &tools[0];
        // Group path = ancestors BELOW the root (sutdump's csp semantics;
        // the root's own name is not a group).
        assert_eq!(map.group_path, vec!["Pen".to_owned()]);
        assert_eq!(
            map.brush.params.get("BrushSize").copied(),
            Some(30.0),
            "the stored 1/100 mm value, verbatim (the app converts)"
        );
        assert_eq!(map.brush.params.get("Opacity"), Some(&100.0));
        let hard = &tools[2];
        assert_eq!(hard.group_path, vec!["Airbrush".to_owned()]);
        assert_eq!(hard.brush.params.get("BrushFlow"), Some(&50.0));
        assert!(
            hard.brush.tip_pngs.is_empty(),
            "tips ride .sut exports (v1)"
        );
    }

    /// The WAL guard: a journal beside the database means a live Clip
    /// Studio and a stale main-file image — refuse loudly, never read.
    #[test]
    fn a_journal_beside_the_database_is_refused() {
        let dir = std::env::temp_dir().join(format!("mn-todb-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("EditImageTool.todb");
        std::fs::write(&db, b"SQLite format 3\0").unwrap();
        assert!(
            parse_todb_file(&db).is_err(),
            "a non-database errors anyway"
        );
        std::fs::write(dir.join("EditImageTool.todb-wal"), b"journal").unwrap();
        let err = match parse_todb_file(&db) {
            Err(e) => e,
            Ok(_) => panic!("the journal must be refused"),
        };
        assert!(err.contains("-wal"), "the refusal names the journal: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The table filter: only Node/Manager/Variant decode. A table the
    /// walk never names is absent even though the file carries it.
    #[test]
    fn only_the_wanted_tables_decode() {
        let Some(b) = fixture() else { return };
        let tables = sqlite_ro::parse_sqlite_tables(&b, &["Node"]).expect("filtered parse");
        assert!(tables.contains_key("Node"));
        assert!(
            !tables.contains_key("Variant"),
            "the filter skipped the un-named table"
        );
        // And the unfiltered read still sees everything (the .sut path).
        let all = sqlite_ro::parse_sqlite(&b).expect("whole parse");
        assert!(all.contains_key("Variant") && all.contains_key("Manager"));
    }
}
