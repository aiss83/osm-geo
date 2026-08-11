#!/bin/bash
# Загрузка ONNX-модели нормализатора (mt5-small, 98.1% точность)
# Использование: bash models/download.sh [RELEASE_TAG]
# По умолчанию скачивает последний релиз.

set -euo pipefail

RELEASE_TAG="${1:-v0.2.0}"
BASE_URL="https://github.com/aiss83/osm-geo/releases/download/${RELEASE_TAG}"
DEST_DIR="$(dirname "$0")"

echo "Загрузка модели нормализатора (${RELEASE_TAG})..."
echo "В директорию: ${DEST_DIR}"
echo ""

FILES=(
    "normalizer_encoder.onnx"
    "normalizer_decoder.onnx"
    "spiece.model"
)

for file in "${FILES[@]}"; do
    url="${BASE_URL}/${file}"
    dest="${DEST_DIR}/${file}"

    if [ -f "$dest" ]; then
        echo "  ${file} — уже существует, пропускаем"
        continue
    fi

    echo "  Скачиваю ${file}..."
    curl -sSL "$url" -o "$dest"
    size=$(du -h "$dest" | cut -f1)
    echo "    Готово: ${size}"
done

echo ""
echo "Проверка:"
ls -lh "${DEST_DIR}"/normalizer_*.onnx "${DEST_DIR}"/spiece.model 2>/dev/null || echo "  (не все файлы загружены)"

echo ""
echo "Модель готова. Сборка: cargo build --release --features neural-normalizer,neural-tokenizer"
