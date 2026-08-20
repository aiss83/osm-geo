# План устранения мелких замечаний (по итогам ревью)

Ниже — пункты «мелочи» из ревью кода с предлагаемыми правками.

## 1. NaN-безопасная сортировка координат

`src/compact.rs`, сортировка Record Block:

```rust
temp_records.sort_by(|a, b| {
    a.0.partial_cmp(&b.0).unwrap().then_with(|| a.1.partial_cmp(&b.1).unwrap())
});
```

Если `lat`/`lon` окажутся `NaN`, `unwrap()` упадёт. Правка: использовать
`f64::total_cmp` (стабильная, NaN-совместимая сортировка):

```rust
temp_records.sort_by(|a, b| {
    a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1))
});
```

## 2. Обрезка `region` по границе UTF-8

`src/compact.rs`:

```rust
let copy_len = region_utf8.len().min(45);
```

При не-ASCII названии региона может разрезать кодпоинт. Правка — обрезать до
ближайшей границы символа:

```rust
let mut copy_len = region_utf8.len().min(45);
while copy_len > 0 && !region_utf8.is_char_boundary(copy_len) {
    copy_len -= 1;
}
```

## 3. Избыточный `if` в corrector.rs

`src/corrector.rs:304` — обе ветки возвращают `"ое"`:

```rust
'n' => format_adj(&adj, &stem, if soft { "ое" } else { "ое" }),
```

Правка: убрать `if`:

```rust
'n' => format_adj(&adj, &stem, "ое"),
```

## 4. Проверка длины токена в FTS

`src/fts.rs` сериализует `token_len` как `u16` без проверки:

```rust
tokens_blob.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
```

Токены — одиночные слова, поэтому переполнение нереально, но дефенсивно стоит
добавить проверку `bytes.len() <= u16::MAX as usize` (или отладочный `assert`).

## 5. Остатки SQLite в `output_stem`

`src/main.rs`, при выборе выходного пути оставалось отрезание `.db`/`.db.zst`:

```rust
let trimmed = s
    .strip_suffix(".db.zst")
    .or_else(|| s.strip_suffix(".db"))
    .or_else(|| s.strip_suffix(".bin"))
    .unwrap_or(&s);
```

Правка (сделано): SQLite-остатки убраны; формат теперь `.osmg`, а `.bin`
оставлен только как legacy-совместимость:

```rust
let trimmed = s
    .strip_suffix(".osmg")
    .or_else(|| s.strip_suffix(".bin"))
    .unwrap_or(&s);
```

## 6. Устаревшие вызовы corrector в parser.rs

`cargo build` даёт 3 deprecated-предупреждения в `src/parser.rs`:

- `Corrector::normalize_street_types_case` → `normalizer::normalize_oblique_street_types`;
- `Corrector::fix_adjective_agreement` → `normalizer::normalize_rule_based`;
- `Corrector::normalize_case` → `normalizer::normalize_rule_based`.

Правка: перенести вызовы на `normalizer` после верификации паритета качества
(см. комментарии в коде «будет удалено после верификации»).

> **Внимание:** это НЕ drop-in-замена. `normalizer::normalize_rule_based` сейчас
> делает только `expand_abbreviations` + `normalize_oblique_street_types` и
> **не покрывает** `fix_adjective_agreement` и `normalize_case`. Миграция без
> переноса этой логики приведёт к регрессу качества. Перед заменой нужно либо
> реализовать эквиваленты в normalizer, либо сохранить эти два шага в
> corrector-вызовах.

## 7. Стилистика clippy

Прогнать `cargo clippy --fix --bin osm-geo` для авто-правок: `map_or` →
`is_none_or`, сворачивание `if`, лишние `&` в паттернах, `doc list item without
indentation` и т.д. Перед авто-правкой убедиться, что тесты проходят, и
просмотреть diff (часть правок может задеть читаемость).
