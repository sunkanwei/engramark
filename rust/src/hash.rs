//! MEMSEM\0 / MEMSET\0v2 / MEMTXN\0v1 encodings. Field numbering, types,
//! lengths, ordering and SHA-256 results are byte-identical to Python.

use sha2::{Digest, Sha256};

use crate::json::Json;
use crate::mem::{canonical_entities, Card};
use crate::normalize::normalize_text;
use crate::MEM_FORMAT_VERSION;
use crate::SOURCE_COLLECTION_HASH_VERSION;

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn sha256_raw(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn field_bytes(out: &mut Vec<u8>, number: u16, kind: u8, data: &[u8]) {
    out.extend_from_slice(&number.to_be_bytes());
    out.push(kind);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

pub fn semantic_hash(card: &Card) -> String {
    let mut payload = b"MEMSEM\0".to_vec();
    let mut push =
        |number: u16, kind: u8, data: &[u8]| field_bytes(&mut payload, number, kind, data);
    push(1, 1, MEM_FORMAT_VERSION.to_string().as_bytes());
    push(2, 1, card.id.to_string().as_bytes());
    push(3, 2, card.card_type.as_bytes());
    push(4, 2, card.status.as_bytes());
    push(5, 1, card.importance.to_string().as_bytes());
    push(6, 1, card.trust.to_string().as_bytes());
    push(7, 2, card.updated.as_bytes());
    push(8, 2, card.source.as_bytes());
    push(9, 1, if card.lock { b"1" } else { b"0" });
    push(10, 2, card.scope.as_bytes());
    push(11, 2, card.last_used.as_bytes());
    push(12, 2, card.valid_from.as_bytes());
    push(13, 2, card.valid_to.as_bytes());
    push(14, 2, card.title.as_bytes());
    push(15, 3, card.body.join("\n").as_bytes());
    for entity in canonical_entities(&card.entities) {
        push(16, 2, normalize_text(&entity).as_bytes());
    }
    let mut refs = card.supersedes.clone();
    refs.sort_unstable();
    refs.dedup();
    for cid in refs {
        push(17, 1, cid.to_string().as_bytes());
    }
    sha256_hex(&payload)
}

/// Hash a (relative_path, bytes) set with MEMSET\0v2.
pub fn source_collection_hash_items(items: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = items.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut payload = format!("MEMSET\0v{SOURCE_COLLECTION_HASH_VERSION}").into_bytes();
    for (rel, data) in sorted {
        field_bytes(&mut payload, 1, 2, rel.as_bytes());
        field_bytes(&mut payload, 2, 1, data.len().to_string().as_bytes());
        field_bytes(&mut payload, 3, 2, sha256_hex(data).as_bytes());
    }
    sha256_hex(&payload)
}

/// SHA-256 over MEMTXN\0v1 + canonical JSON without the checksum field.
pub fn journal_checksum(payload: &Json) -> String {
    let mut unsigned = Vec::new();
    if let Json::Object(pairs) = payload {
        for (key, value) in pairs {
            if key != "checksum" {
                unsigned.push((key.clone(), value.clone()));
            }
        }
    }
    let raw = Json::Object(unsigned).dumps_canonical();
    sha256_hex(&format!("MEMTXN\0v1{raw}").into_bytes())
}
