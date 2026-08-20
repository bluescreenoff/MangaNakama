/* MangaNakama tile surface shim.
 *
 * This is *our* code, not vendored — it is the C half of the
 * `MyPaintTiledSurface` subclass whose tile store lives in Rust
 * (`crates/brush/src/surface.rs`). It is a direct transliteration of
 * `vendor/libmypaint/mypaint-fixed-tiled-surface.c`, with the linear tile
 * buffer swapped for two callbacks into Rust.
 *
 * Why a C shim instead of a `#[repr(C)]` mirror of `MyPaintTiledSurface` in
 * Rust: subclassing means embedding `MyPaintTiledSurface` (which itself embeds
 * `MyPaintSurface`) as the first member, so the layout must match the C
 * compiler's exactly. Getting that wrong is silent memory corruption, not a
 * compile error. Here the C compiler computes the layout, and Rust only ever
 * sees two opaque pointers. Zero hand-maintained struct layouts.
 */

#include <stdint.h>
#include <stdlib.h>

#include "mypaint-tiled-surface.h"

typedef struct {
    MyPaintTiledSurface parent;
    void* rust_state; /* &mut SurfaceState, owned by Rust */
} MnSurface;

/* Implemented in Rust (crates/brush/src/surface.rs).
 *
 * `mn_brush_tile_request_start` must return a pointer to 64*64*4 uint16_t that
 * stays valid until the matching `..._end`. libmypaint only ever has one tile
 * checked out at a time on this path (`threadsafe_tile_requests` is FALSE and
 * we never compile with OpenMP), which is what makes that safe. */
extern uint16_t* mn_brush_tile_request_start(void* state, int tx, int ty, int readonly);
extern void mn_brush_tile_request_end(void* state, int tx, int ty, int readonly);

static void
mn_tile_request_start(MyPaintTiledSurface* tiled_surface, MyPaintTileRequest* request)
{
    MnSurface* self = (MnSurface*)tiled_surface;
    request->buffer =
        mn_brush_tile_request_start(self->rust_state, request->tx, request->ty, request->readonly ? 1 : 0);
}

static void
mn_tile_request_end(MyPaintTiledSurface* tiled_surface, MyPaintTileRequest* request)
{
    MnSurface* self = (MnSurface*)tiled_surface;
    mn_brush_tile_request_end(self->rust_state, request->tx, request->ty, request->readonly ? 1 : 0);
}

/* The MyPaintSurface::destroy vfunc. Rust owns the allocation and calls
 * mn_surface_free directly, so refcount-driven destruction is a no-op. */
static void
mn_surface_destroy_vfunc(MyPaintSurface* surface)
{
    (void)surface;
}

MnSurface*
mn_surface_new(void* rust_state)
{
    MnSurface* self = (MnSurface*)malloc(sizeof(MnSurface));
    if (!self) {
        return NULL;
    }
    mypaint_tiled_surface_init(&self->parent, mn_tile_request_start, mn_tile_request_end);
    self->parent.parent.destroy = mn_surface_destroy_vfunc;
    self->rust_state = rust_state;
    return self;
}

void
mn_surface_set_state(MnSurface* self, void* rust_state)
{
    self->rust_state = rust_state;
}

MyPaintSurface*
mn_surface_interface(MnSurface* self)
{
    return (MyPaintSurface*)self;
}

void
mn_surface_free(MnSurface* self)
{
    if (!self) {
        return;
    }
    mypaint_tiled_surface_destroy(&self->parent);
    free(self);
}

/* Size sanity: the Rust side assumes libmypaint's native tile geometry so that
 * core::Tile buffers can be handed over with zero conversion. If either of
 * these ever stops holding, this fails at compile time instead of at runtime. */
typedef char mn_assert_tile_size[(MYPAINT_TILE_SIZE == 64) ? 1 : -1];
typedef char mn_assert_u16[(sizeof(uint16_t) == 2) ? 1 : -1];
