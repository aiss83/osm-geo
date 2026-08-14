// C ABI поверх libgeodesk (C++), чтобы читать GOL из Rust без подпроцесса
// и без промежуточного PBF.
//
// Лицензия libgeodesk — LGPL-3.0-only (см. vendor/libgeodesk/LICENSE).

#include <geodesk/geodesk.h>

#include <optional>
#include <string>
#include <utility>

#include "gol_ffi.h"

using namespace geodesk;

// Обратите внимание: структуры определены в глобальном namespace,
// чтобы совпадать с opaque-typedef'ами из gol_ffi.h.

struct GolFeatures {
    Features features;

    explicit GolFeatures(const char* path) : features(path) {}
    explicit GolFeatures(Features&& f) : features(std::move(f)) {}
};

struct GolFeature {
    Features collection;  // владеет хранилищем через refcount
    FeatureIterator<Feature> it;
    std::optional<Feature> current;  // у Feature нет default-ctor и copy-assign
    std::string scratch;

    explicit GolFeature(const GolFeatures* f)
        : collection(f->features), it(collection.begin()) {}

    bool next() {
        if (it == nullptr) {
            return false;
        }
        current.emplace(*it);
        ++it;
        return true;
    }

    const char* tag(const char* key) {
        Tags tags = current->tags();
        if (!tags.hasTag(key)) {
            return nullptr;
        }
        TagValue value = tags[key];
        scratch = static_cast<std::string>(value);
        return scratch.c_str();
    }
};

extern "C" {

GolFeatures* gol_open(const char* path) {
    try {
        return new GolFeatures(path);
    } catch (...) {
        return nullptr;
    }
}

void gol_close(GolFeatures* f) {
    delete f;
}

GolFeatures* gol_query(const GolFeatures* f, const char* query) {
    try {
        return new GolFeatures(f->features(query));
    } catch (...) {
        return nullptr;
    }
}

GolFeature* gol_iterate(const GolFeatures* f) {
    try {
        return new GolFeature(f);
    } catch (...) {
        return nullptr;
    }
}

int gol_next(GolFeature* it) {
    return it->next() ? 1 : 0;
}

void gol_free(GolFeature* it) {
    delete it;
}

int64_t gol_id(const GolFeature* it) {
    return it->current->id();
}

int gol_type(const GolFeature* it) {
    const Feature& f = *it->current;
    if (f.isNode()) return 0;
    if (f.isWay()) return 1;
    return 2;
}

double gol_lon(const GolFeature* it) {
    return it->current->centroid().lon();
}

double gol_lat(const GolFeature* it) {
    return it->current->centroid().lat();
}

const char* gol_tag(GolFeature* it, const char* key) {
    return it->tag(key);
}

}  // extern "C"
