//! Инвертированный полнотекстовый индекс для компактного формата.
//!
//! Структура на диске (см. `docs/PLAN_compact_fts.md`):
//!
//! - **словарь токенов**: `u32 count`, затем для каждого токена
//!   `u16 token_len` + байты токена + `u32 postings_offset` + `u32 postings_count`;
//! - **постинг-листы**: `u32 total_count`, затем для каждого токена список
//!   `record_idx`, закодированный дельтами в varint (LEB128).

use std::collections::HashMap;

/// Строит инвертированный индекс `токен → [record_idx]`.
pub struct FtsIndex {
    map: HashMap<String, Vec<u32>>,
}

impl FtsIndex {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn add(&mut self, token: String, record_idx: u32) {
        self.map.entry(token).or_default().push(record_idx);
    }

    pub fn token_count(&self) -> usize {
        self.map.len()
    }

    pub fn posting_count(&self) -> usize {
        self.map.values().map(Vec::len).sum()
    }

    /// Сериализовать индекс в `(tokens_blob, postings_blob)`.
    ///
    /// Сортирует токены и постинг-листы, убирает дубликаты внутри одного
    /// постинг-листа, кодирует `record_idx` дельтами через varint.
    pub fn serialize(&mut self) -> (Vec<u8>, Vec<u8>) {
        let mut tokens: Vec<String> = self.map.keys().cloned().collect();
        tokens.sort();

        let mut tokens_blob = Vec::new();
        tokens_blob.extend_from_slice(&(tokens.len() as u32).to_le_bytes());

        // Первые 4 байта — общее число постингов; заполним после прохода.
        let mut postings_blob = Vec::new();
        postings_blob.extend_from_slice(&0u32.to_le_bytes());

        let mut total_postings = 0u32;

        for token in &tokens {
            let postings = self.map.get_mut(token).expect("token present");
            postings.sort_unstable();
            postings.dedup();

            let postings_offset = postings_blob.len() as u32;
            let postings_count = postings.len() as u32;

            let mut prev = 0u32;
            for &record_idx in postings.iter() {
                // Постинги отсортированы по возрастанию, дельта >= 0.
                write_varint(&mut postings_blob, record_idx - prev);
                prev = record_idx;
            }

            let bytes = token.as_bytes();
            debug_assert!(bytes.len() <= u16::MAX as usize, "FTS-токен слишком длинный");
            tokens_blob.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
            tokens_blob.extend_from_slice(bytes);
            tokens_blob.extend_from_slice(&postings_offset.to_le_bytes());
            tokens_blob.extend_from_slice(&postings_count.to_le_bytes());

            total_postings += postings_count;
        }

        postings_blob[0..4].copy_from_slice(&total_postings.to_le_bytes());

        (tokens_blob, postings_blob)
    }
}

/// Закодировать `u32` как unsigned LEB128.
fn write_varint(buf: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_varint(data: &[u8], pos: &mut usize) -> u32 {
        let mut value = 0u32;
        let mut shift = 0u32;
        loop {
            let byte = data[*pos];
            *pos += 1;
            value |= ((byte & 0x7f) as u32) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    }

    #[test]
    fn test_serialize_roundtrip() {
        let mut idx = FtsIndex::new();
        idx.add("москв".to_string(), 5);
        idx.add("москв".to_string(), 1);
        idx.add("москв".to_string(), 3);
        idx.add("тверск".to_string(), 2);

        let (tokens_blob, postings_blob) = idx.serialize();

        // Словарь: count + 2 токена
        let count = u32::from_le_bytes(tokens_blob[0..4].try_into().unwrap());
        assert_eq!(count, 2);

        let mut pos = 4usize;
        let mut decoded = Vec::new();
        for _ in 0..count {
            let len = u16::from_le_bytes(tokens_blob[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let token = String::from_utf8(tokens_blob[pos..pos + len].to_vec()).unwrap();
            pos += len;
            let postings_offset =
                u32::from_le_bytes(tokens_blob[pos..pos + 4].try_into().unwrap());
            pos += 4;
            let postings_count =
                u32::from_le_bytes(tokens_blob[pos..pos + 4].try_into().unwrap());
            pos += 4;

            let mut p = postings_offset as usize;
            let mut prev = 0u32;
            let mut list = Vec::new();
            for _ in 0..postings_count {
                let rec = read_varint(&postings_blob, &mut p) + prev;
                prev = rec;
                list.push(rec);
            }
            decoded.push((token, list));
        }

        assert_eq!(
            decoded,
            vec![
                ("москв".to_string(), vec![1, 3, 5]),
                ("тверск".to_string(), vec![2]),
            ]
        );

        // Общее число постингов.
        let total = u32::from_le_bytes(postings_blob[0..4].try_into().unwrap());
        assert_eq!(total, 4);
    }
}
