//! Tiles: the one memory layout the whole app agrees on.
//!
//! Pinned by docs/ARCHITECTURE.md, do not renegotiate:
//!   64x64 px, RGBA u16, **premultiplied alpha**, fix15 (`1.0 == 1<<15 == 32768`).
//! That is libmypaint's native surface format, so the brush hot path does zero
//! conversion. Display may approximate fix15 -> unorm with a shader scale;
//! export/save paths must convert exactly on the CPU.

use std::sync::atomic::{AtomicU64, Ordering};

/// Tile edge length in pixels.
pub const TILE_SIZE: usize = 64;
/// Pixels per tile.
pub const TILE_PIXELS: usize = TILE_SIZE * TILE_SIZE;
/// Channels per pixel (RGBA).
pub const TILE_CHANNELS: usize = 4;
/// `u16` elements in a tile's backing buffer.
pub const TILE_LEN: usize = TILE_PIXELS * TILE_CHANNELS;
/// fix15 unity: `1.0 == 32768`.
pub const FIX15_ONE: u32 = 1 << 15;

/// Process-global monotonic revision source.
///
/// Deliberately global rather than per-layer: undo (a later agent) restores old
/// `Arc<Tile>` snapshots, and a per-layer counter would hand back a revision the
/// GPU cache has already seen, silently skipping the upload. Allocating from one
/// ever-increasing counter makes "newer number == needs upload" unconditionally
/// true. Starts at 1 so 0 is a usable "never seen" sentinel for caches.
static REVISION: AtomicU64 = AtomicU64::new(1);

/// Allocate the next globally-unique revision number.
pub fn next_revision() -> u64 {
    REVISION.fetch_add(1, Ordering::Relaxed)
}

/// Index of a tile in canvas tile space. Canvas origin is top-left, y-down;
/// `tile = floor(pixel / 64)` (floor, not truncate — negatives matter once
/// selections/transforms can push content off-canvas).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TileIdx {
    pub x: i32,
    pub y: i32,
}

impl TileIdx {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Tile containing the given canvas pixel.
    pub const fn of_pixel(px: i32, py: i32) -> Self {
        Self {
            x: px.div_euclid(TILE_SIZE as i32),
            y: py.div_euclid(TILE_SIZE as i32),
        }
    }

    /// Canvas-pixel coordinate of this tile's top-left corner.
    pub const fn origin(self) -> (i32, i32) {
        (self.x * TILE_SIZE as i32, self.y * TILE_SIZE as i32)
    }
}

/// A 64x64 RGBA-u16 premultiplied fix15 tile.
///
/// The buffer is heap-allocated (`Box<[u16]>` built from a `Vec`, never a boxed
/// array literal — a `[u16; 16384]` temporary on the stack is 32 KiB and blows
/// up in debug builds).
#[derive(Clone)]
pub struct Tile {
    data: Box<[u16]>,
    rev: u64,
}

impl Tile {
    /// A fully transparent tile carrying a fresh revision.
    pub fn new_transparent() -> Self {
        Self {
            data: vec![0u16; TILE_LEN].into_boxed_slice(),
            rev: next_revision(),
        }
    }

    /// Raw fix15 premultiplied RGBA, row-major, `TILE_LEN` elements.
    #[inline]
    pub fn data(&self) -> &[u16] {
        &self.data
    }

    /// Mutable raw access. Does **not** bump the revision — `Layer::tile_mut`
    /// owns that, so every mutation goes through one place the GPU can trust.
    #[inline]
    pub fn data_mut(&mut self) -> &mut [u16] {
        &mut self.data
    }

    /// Monotonic revision; GPU caches upload only when this exceeds theirs.
    #[inline]
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// Stamp a fresh revision on this tile.
    #[inline]
    pub fn touch(&mut self) {
        self.rev = next_revision();
    }

    /// Index of pixel `(x, y)` (tile-local) into `data`.
    #[inline]
    pub const fn offset(x: usize, y: usize) -> usize {
        (y * TILE_SIZE + x) * TILE_CHANNELS
    }

    /// Read one premultiplied fix15 pixel.
    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> [u16; 4] {
        let o = Self::offset(x, y);
        [
            self.data[o],
            self.data[o + 1],
            self.data[o + 2],
            self.data[o + 3],
        ]
    }

    /// Write one premultiplied fix15 pixel.
    #[inline]
    pub fn set_pixel(&mut self, x: usize, y: usize, px: [u16; 4]) {
        let o = Self::offset(x, y);
        self.data[o..o + 4].copy_from_slice(&px);
    }

    /// True when every channel is zero (fully transparent).
    pub fn is_blank(&self) -> bool {
        self.data.iter().all(|&v| v == 0)
    }

    /// Sum of the alpha channel — cheap "did anything land here" probe for tests.
    pub fn alpha_sum(&self) -> u64 {
        self.data
            .chunks_exact(TILE_CHANNELS)
            .map(|p| u64::from(p[3]))
            .sum()
    }
}

impl Default for Tile {
    fn default() -> Self {
        Self::new_transparent()
    }
}

impl std::fmt::Debug for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tile")
            .field("rev", &self.rev)
            .field("blank", &self.is_blank())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_is_transparent_and_sized() {
        let t = Tile::new_transparent();
        assert_eq!(t.data().len(), TILE_LEN);
        assert!(t.is_blank());
        assert_eq!(t.alpha_sum(), 0);
    }

    #[test]
    fn revisions_are_monotonic() {
        let a = next_revision();
        let b = next_revision();
        assert!(b > a);
    }

    #[test]
    fn tile_index_floors_toward_negative_infinity() {
        assert_eq!(TileIdx::of_pixel(0, 0), TileIdx::new(0, 0));
        assert_eq!(TileIdx::of_pixel(63, 63), TileIdx::new(0, 0));
        assert_eq!(TileIdx::of_pixel(64, 128), TileIdx::new(1, 2));
        // truncating division would give 0 here; floor must give -1.
        assert_eq!(TileIdx::of_pixel(-1, -1), TileIdx::new(-1, -1));
        assert_eq!(TileIdx::of_pixel(-64, -65), TileIdx::new(-1, -2));
        assert_eq!(TileIdx::new(2, 3).origin(), (128, 192));
    }
}
