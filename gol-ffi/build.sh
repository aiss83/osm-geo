#!/usr/bin/env bash
# Сборка статической библиотеки golffi (libgeodesk + gol_ffi.cpp).
#
# Требования:
#   - clang++ или g++ с поддержкой C++20
#   - исходники libgeodesk (https://github.com/clarisma/libgeodesk)
#   - заголовки gtl (https://github.com/greg7mdp/gtl)
#
# Переменные окружения:
#   LIBGEODESK_DIR — путь к libgeodesk (по умолчанию: /tmp/libgeodesk)
#   GTL_DIR        — путь к gtl (по умолчанию: /tmp/gtl)
#   BUILD_DIR      — каталог сборки (по умолчанию: ./build)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIBGEODESK_DIR="${LIBGEODESK_DIR:-/tmp/libgeodesk}"
GTL_DIR="${GTL_DIR:-/tmp/gtl}"
BUILD_DIR="${BUILD_DIR:-$SCRIPT_DIR/build}"
OUT="${1:-$BUILD_DIR/libgolffi.a}"

CXX="${CXX:-clang++}"
NPROC="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)"

if [ ! -d "$LIBGEODESK_DIR/include" ] || [ ! -d "$LIBGEODESK_DIR/src" ]; then
    echo "Ошибка: LIBGEODESK_DIR не указывает на исходники libgeodesk: $LIBGEODESK_DIR" >&2
    exit 1
fi
if [ ! -d "$GTL_DIR/include" ]; then
    echo "Ошибка: GTL_DIR не указывает на gtl: $GTL_DIR" >&2
    exit 1
fi

mkdir -p "$BUILD_DIR/obj"

echo "Компиляция libgeodesk ($LIBGEODESK_DIR/src)…"
find "$LIBGEODESK_DIR/src" -name '*.cpp' -print0 | \
    xargs -0 -P "$NPROC" -I{} bash -c '
        f="$1"
        o="$2/$(basename "${f%.cpp}").o"
        "$3" -std=c++20 -O2 -I"$4/include" -I"$4/src" -I"$5/include" -c "$f" -o "$o"
    ' _ {} "$BUILD_DIR/obj" "$CXX" "$LIBGEODESK_DIR" "$GTL_DIR"

echo "Компиляция gol_ffi.cpp…"
"$CXX" -std=c++20 -O2 \
    -I"$LIBGEODESK_DIR/include" -I"$LIBGEODESK_DIR/src" -I"$GTL_DIR/include" \
    -c "$SCRIPT_DIR/gol_ffi.cpp" -o "$BUILD_DIR/obj/gol_ffi.o"

echo "Архивирование $OUT…"
ar rcs "$OUT" "$BUILD_DIR"/obj/*.o
echo "Готово: $OUT"
