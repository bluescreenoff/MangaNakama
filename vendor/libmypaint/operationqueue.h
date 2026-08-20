#ifndef OPERATIONQUEUE_H
#define OPERATIONQUEUE_H

#include <stdint.h>
#include "tilemap.h"

typedef struct {
    float x;
    float y;
    float radius;
    uint16_t color_r;
    uint16_t color_g;
    uint16_t color_b;
    float color_a;
    float opaque;
    float hardness;
    float aspect_ratio;
    float angle;
    float normal;
    float lock_alpha;
    float colorize;
    float posterize;
    float posterize_num;
    float paint;
    /* mnc (#0.1, 2026-08-17): texture-tip scroll offset snapshot taken at
     * DRAW time. The op queue defers rendering to tile-process time, when
     * the crawl accumulator has advanced past this dab — without the
     * snapshot every dab of one end_atomic batch rendered at the batch's
     * final offset (per-sample crawl, not the per-dab crawl patch #10
     * documents). */
    float tex_dx;
    float tex_dy;
} OperationDataDrawDab;

typedef struct OperationQueue OperationQueue;

OperationQueue *operation_queue_new(void);
void operation_queue_free(OperationQueue *self);

int operation_queue_get_dirty_tiles(OperationQueue *self, TileIndex** tiles_out);
void operation_queue_clear_dirty_tiles(OperationQueue *self);

void operation_queue_add(OperationQueue *self, TileIndex index, OperationDataDrawDab *op);
OperationDataDrawDab *operation_queue_pop(OperationQueue *self, TileIndex index);

OperationDataDrawDab *operation_queue_peek_first(OperationQueue *self, TileIndex index);
OperationDataDrawDab *operation_queue_peek_last(OperationQueue *self, TileIndex index);

#endif // OPERATIONQUEUE_H
