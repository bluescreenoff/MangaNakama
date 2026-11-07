//! Layered PSD/PSB import and export, built on the vendored
//! `photocraft-psd` crate (`vendor/photocraft-psd/VENDOR.md`).
//!
//! Both directions carry pixels, folders, opacity, the 27 blend modes,
//! visibility, clipping, layer masks, the lock flags and the resolution.
//! Text, effects, adjustment layers and smart objects travel as the pixels
//! they store; nothing is made editable on either side.
//!
//! - [`save_psd`]: 8-bit RGB, RLE, the export composite as the merged image.
//!   A canvas over 30,000 px per side is written as PSB ([`needs_psb`]).
//! - [`load`] / [`load_from_bytes`]: any colour mode and depth the crate
//!   decodes, reduced to 8-bit straight RGBA, into a bare single page. What
//!   did not survive is listed in [`PsdImport::notes`].
//!
//! The previous writer lives on as `psd_legacy` for one release, behind the
//! app's `psd_legacy_writer` preference.

mod blend;
mod export;
mod import;
#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::Path;

use crate::doc::Document;

#[derive(Debug)]
pub struct PsdError(pub String);

impl std::fmt::Display for PsdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "psd: {}", self.0)
    }
}

impl From<photocraft_psd::PsdError> for PsdError {
    fn from(e: photocraft_psd::PsdError) -> Self {
        PsdError(e.to_string())
    }
}

/// A PSD read into a document, plus one line per thing that did not come
/// across as it was in the file (for the status line).
pub struct PsdImport {
    pub doc: Document,
    pub notes: Vec<String>,
}

/// PSD's per-side limit; past it the file has to be PSB.
const PSD_MAX_SIDE: u32 = 30_000;

/// True when a canvas this size cannot be a plain PSD.
pub fn needs_psb(size: (u32, u32)) -> bool {
    size.0 > PSD_MAX_SIDE || size.1 > PSD_MAX_SIDE
}

/// Write `doc` as a layered PSD (PSB when [`needs_psb`]).
pub fn save_psd<W: Write>(doc: &Document, mut out: W) -> Result<(), PsdError> {
    let bytes = export::to_bytes(doc)?;
    out.write_all(&bytes).map_err(|e| PsdError(e.to_string()))
}

/// Read a PSD or PSB file.
pub fn load(path: &Path) -> Result<PsdImport, PsdError> {
    let bytes = std::fs::read(path).map_err(|e| PsdError(e.to_string()))?;
    load_from_bytes(&bytes)
}

/// Read a PSD or PSB held in memory.
pub fn load_from_bytes(bytes: &[u8]) -> Result<PsdImport, PsdError> {
    import::load_from_bytes(bytes)
}
