// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Low-level encodings shared by the block writer/reader: varint/zigzag/delta
//! integer packing, length-prefixed string sequences, a small bloom filter, and
//! the per-section zstd compression wrapper (toggleable).

use anyhow::{anyhow, bail, Result};

use super::Codec;

// varint / zigzag

#[inline]
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[inline]
pub fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

pub fn write_uvarint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut byte = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
}

pub fn read_uvarint(buf: &mut &[u8]) -> Result<u64> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let (&byte, rest) = buf
            .split_first()
            .ok_or_else(|| anyhow!("uvarint: unexpected end of buffer"))?;
        *buf = rest;
        if shift >= 64 {
            bail!("uvarint: overflow");
        }
        result |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    Ok(result)
}

// i64 columns

/// Delta + zigzag + varint. Tuned for ascending, near-monotonic data
/// (timestamps): consecutive deltas are tiny, so most values become 1 byte.
pub fn encode_i64_delta(vals: &[i64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vals.len() + 8);
    let mut prev: i64 = 0;
    for &v in vals {
        let delta = v.wrapping_sub(prev);
        write_uvarint(&mut out, zigzag(delta));
        prev = v;
    }
    out
}

pub fn decode_i64_delta(mut buf: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut out = Vec::with_capacity(count);
    let mut prev: i64 = 0;
    for _ in 0..count {
        let delta = unzigzag(read_uvarint(&mut buf)?);
        prev = prev.wrapping_add(delta);
        out.push(prev);
    }
    Ok(out)
}

/// Plain zigzag + varint — no delta. For unsorted numeric value columns where
/// delta buys nothing.
pub fn encode_i64_plain(vals: &[i64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vals.len() + 8);
    for &v in vals {
        write_uvarint(&mut out, zigzag(v));
    }
    out
}

pub fn decode_i64_plain(mut buf: &[u8], count: usize) -> Result<Vec<i64>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(unzigzag(read_uvarint(&mut buf)?));
    }
    Ok(out)
}

// f64 columns

pub fn encode_f64(vals: &[f64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vals.len() * 8);
    for &v in vals {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn decode_f64(buf: &[u8], count: usize) -> Result<Vec<f64>> {
    if buf.len() < count * 8 {
        bail!("f64 column: short buffer ({} < {})", buf.len(), count * 8);
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let mut b = [0u8; 8];
        b.copy_from_slice(&buf[i * 8..i * 8 + 8]);
        out.push(f64::from_le_bytes(b));
    }
    Ok(out)
}

// string sequences

/// Encode an ordered list of optional strings: per entry, `uvarint(len+1)` (0 ⇒
/// absent) then the UTF-8 bytes. Reconstructs a `Vec<Option<String>>` exactly.
pub fn encode_opt_strings(vals: &[Option<&str>]) -> Vec<u8> {
    let mut out = Vec::new();
    for v in vals {
        match v {
            None => write_uvarint(&mut out, 0),
            Some(s) => {
                write_uvarint(&mut out, s.len() as u64 + 1);
                out.extend_from_slice(s.as_bytes());
            }
        }
    }
    out
}

pub fn decode_opt_strings(mut buf: &[u8], count: usize) -> Result<Vec<Option<String>>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = read_uvarint(&mut buf)?;
        if tag == 0 {
            out.push(None);
            continue;
        }
        let len = (tag - 1) as usize;
        if buf.len() < len {
            bail!("string column: short buffer");
        }
        let s = std::str::from_utf8(&buf[..len])
            .map_err(|e| anyhow!("string column: invalid utf8: {e}"))?
            .to_string();
        buf = &buf[len..];
        out.push(Some(s));
    }
    Ok(out)
}

/// Decode only the rows in `wanted` (ascending, unique, `< count`) from an
/// opt-string section, returning their values in `wanted` order. Walks the
/// sequential format once but allocates a `String` only for wanted rows — so
/// pulling K rows from an N-row column costs K allocations, not N.
pub fn decode_opt_strings_subset(
    mut buf: &[u8],
    count: usize,
    wanted: &[u32],
) -> Result<Vec<Option<String>>> {
    let mut out = Vec::with_capacity(wanted.len());
    let mut wi = 0usize;
    for i in 0..count {
        if wi >= wanted.len() {
            break;
        }
        let tag = read_uvarint(&mut buf)?;
        let len = if tag == 0 { 0 } else { (tag - 1) as usize };
        if buf.len() < len {
            bail!("string column: short buffer");
        }
        if wanted[wi] as usize == i {
            out.push((tag != 0).then(|| {
                // utf8 was validated at encode time; lossy keeps this infallible.
                String::from_utf8_lossy(&buf[..len]).into_owned()
            }));
            wi += 1;
        }
        buf = &buf[len..];
    }
    Ok(out)
}

/// Encode a sorted term dictionary: `uvarint(count)` then per term
/// `uvarint(len)` + bytes. Read with [`decode_dict`].
pub fn encode_dict<S: AsRef<str>>(terms: &[S]) -> Vec<u8> {
    let mut out = Vec::new();
    write_uvarint(&mut out, terms.len() as u64);
    for t in terms {
        let t = t.as_ref();
        write_uvarint(&mut out, t.len() as u64);
        out.extend_from_slice(t.as_bytes());
    }
    out
}

pub fn decode_dict(mut buf: &[u8]) -> Result<Vec<String>> {
    let count = read_uvarint(&mut buf)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let len = read_uvarint(&mut buf)? as usize;
        if buf.len() < len {
            bail!("dict: short buffer");
        }
        let s = std::str::from_utf8(&buf[..len])
            .map_err(|e| anyhow!("dict: invalid utf8: {e}"))?
            .to_string();
        buf = &buf[len..];
        out.push(s);
    }
    Ok(out)
}

// bloom filter

/// Small fixed-`k` bloom over a field's terms, for "term possibly here?" block-skip.
/// Serialized as `uvarint(num_u64_words)` + LE words; `k` is fixed at 7.
pub struct Bloom {
    bits: Vec<u64>,
    nbits: u64,
}

const BLOOM_K: u32 = 7;

impl Bloom {
    /// Size to ~10 bits/term (≈1% FP at k=7), min one word.
    pub fn with_capacity(n_terms: usize) -> Self {
        let words = (n_terms * 10 / 64).max(1);
        Bloom {
            bits: vec![0u64; words],
            nbits: (words * 64) as u64,
        }
    }

    fn hashes(term: &str) -> (u64, u64) {
        let h1 = fnv1a(term.as_bytes());
        // Second independent hash via a salted FNV (double hashing).
        let h2 = fnv1a_seed(term.as_bytes(), 0x9e3779b97f4a7c15);
        (h1, h2 | 1)
    }

    pub fn insert(&mut self, term: &str) {
        let (h1, h2) = Self::hashes(term);
        for i in 0..BLOOM_K as u64 {
            let bit = h1.wrapping_add(h2.wrapping_mul(i)) % self.nbits;
            self.bits[(bit / 64) as usize] |= 1u64 << (bit % 64);
        }
    }

    pub fn contains(&self, term: &str) -> bool {
        let (h1, h2) = Self::hashes(term);
        for i in 0..BLOOM_K as u64 {
            let bit = h1.wrapping_add(h2.wrapping_mul(i)) % self.nbits;
            if self.bits[(bit / 64) as usize] & (1u64 << (bit % 64)) == 0 {
                return false;
            }
        }
        true
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.bits.len() * 8 + 4);
        write_uvarint(&mut out, self.bits.len() as u64);
        for w in &self.bits {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn deserialize(mut buf: &[u8]) -> Result<Bloom> {
        let words = read_uvarint(&mut buf)? as usize;
        if buf.len() < words * 8 {
            bail!("bloom: short buffer");
        }
        let mut bits = Vec::with_capacity(words);
        for i in 0..words {
            let mut b = [0u8; 8];
            b.copy_from_slice(&buf[i * 8..i * 8 + 8]);
            bits.push(u64::from_le_bytes(b));
        }
        let nbits = (words * 64) as u64;
        Ok(Bloom { bits, nbits })
    }
}

fn fnv1a(data: &[u8]) -> u64 {
    fnv1a_seed(data, 0xcbf29ce484222325)
}

fn fnv1a_seed(data: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// compression (per-section, toggleable)

/// zstd level for the default `Zstd` codec. Low level: good ratio, fast.
const ZSTD_LEVEL: i32 = 3;

/// Compress one section under `codec`. `None` is the benchmark/off path — the
/// type-aware encoding above still ran; only this general pass is skipped.
pub fn compress(codec: Codec, raw: &[u8]) -> Result<Vec<u8>> {
    match codec {
        Codec::None => Ok(raw.to_vec()),
        Codec::Zstd => {
            zstd::stream::encode_all(raw, ZSTD_LEVEL).map_err(|e| anyhow!("zstd compress: {e}"))
        }
    }
}

pub fn decompress(codec: Codec, stored: &[u8], raw_len: u32) -> Result<Vec<u8>> {
    match codec {
        Codec::None => Ok(stored.to_vec()),
        Codec::Zstd => {
            let out = zstd::stream::decode_all(stored).map_err(|e| anyhow!("zstd decode: {e}"))?;
            if out.len() != raw_len as usize {
                bail!(
                    "zstd decode: length mismatch (got {}, footer says {})",
                    out.len(),
                    raw_len
                );
            }
            Ok(out)
        }
    }
}
