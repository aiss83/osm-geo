#ifndef GOL_FFI_H
#define GOL_FFI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct GolFeatures GolFeatures;
typedef struct GolFeature GolFeature;

/* Open a GOL file. Returns NULL on error. */
GolFeatures* gol_open(const char* path);

/* Free a features collection (opened GOL or a query result). */
void gol_close(GolFeatures* f);

/* Run a GOQL query; returns a new collection, or NULL on error.
   The returned collection shares the store with `f`; keep `f` alive
   or the store will be retained by any iterator derived from the result. */
GolFeatures* gol_query(const GolFeatures* f, const char* query);

/* Create an iterator over a collection. Returns NULL on error. */
GolFeature* gol_iterate(const GolFeatures* f);

/* Advance to the next feature. Returns 1 if a feature is available, 0 at end. */
int gol_next(GolFeature* it);

/* Free the iterator. */
void gol_free(GolFeature* it);

/* Feature id. */
int64_t gol_id(const GolFeature* it);

/* Feature type: 0=node, 1=way, 2=relation. */
int gol_type(const GolFeature* it);

/* Feature centroid, WGS-84 degrees. */
double gol_lon(const GolFeature* it);
double gol_lat(const GolFeature* it);

/* Tag lookup by key. Returns NULL if the tag is absent.
   The pointer is valid until the next gol_next() or gol_free(). */
const char* gol_tag(GolFeature* it, const char* key);

#ifdef __cplusplus
}
#endif

#endif
