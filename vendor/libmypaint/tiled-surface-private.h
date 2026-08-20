

/* mnc (round 26): tile_tx/tile_ty feed the canvas-anchored texture-tip
 * sampling (vendor/PATCHES.md #10); pass 0,0 when there is no tile context.
 * mnc (#0.1, 2026-08-17): tex_dx/tex_dy are the dab's DRAW-TIME crawl
 * offsets — the caller snapshots them (the op struct carries them; get_color
 * reads the accumulator directly, running at draw time). */
void render_dab_mask (uint16_t * mask,
                        float x, float y,
                        float radius,
                        float hardness,
                        float aspect_ratio, float angle,
                        int tile_tx, int tile_ty,
                        float tex_dx, float tex_dy
                        );
