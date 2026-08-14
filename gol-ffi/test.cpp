// Smoke test для gol_ffi: открыть GOL, выполнить запрос, пройтись по фичам.
#include <cstdio>
#include <cstdlib>

#include "gol_ffi.h"

int main(int argc, char** argv) {
    if (argc < 2) {
        fprintf(stderr, "Usage: %s <file.gol> [query]\n", argv[0]);
        return 2;
    }
    const char* path = argv[1];
    const char* query = argc > 2 ? argv[2] : "*";

    GolFeatures* lib = gol_open(path);
    if (!lib) {
        fprintf(stderr, "gol_open failed\n");
        return 1;
    }

    GolFeatures* subset = gol_query(lib, query);
    if (!subset) {
        fprintf(stderr, "gol_query failed\n");
        gol_close(lib);
        return 1;
    }

    GolFeature* it = gol_iterate(subset);
    if (!it) {
        fprintf(stderr, "gol_iterate failed\n");
        gol_close(subset);
        gol_close(lib);
        return 1;
    }

    long count = 0;
    while (gol_next(it)) {
        const char* name = gol_tag(it, "name");
        printf("%lld (%.6f, %.6f) name=%s\n",
               (long long)gol_id(it), gol_lon(it), gol_lat(it),
               name ? name : "(none)");
        if (++count >= 10) break;
    }

    printf("... total shown: %ld\n", count);

    gol_free(it);
    gol_close(subset);
    gol_close(lib);
    return 0;
}
