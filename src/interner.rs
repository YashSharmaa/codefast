//! Concurrent name interner used while building an index.
//!
//! Workers intern names from many files at once and get back *provisional* ids. When the build
//! finishes, `finalize` sorts all names and returns the final ids in byte order, so the on-disk
//! name table is a sorted array that supports binary search and prefix scans without any
//! auxiliary structure.

use crate::facts::hash_bytes;
use hashbrown::HashTable;
use rayon::prelude::*;
use std::sync::Mutex;

const SHARD_BITS: u32 = 7;
const SHARDS: usize = 1 << SHARD_BITS;

#[derive(Default)]
struct Shard {
    table: HashTable<u32>,
    buf: Vec<u8>,
    spans: Vec<(u32, u32)>,
}

pub struct Interner {
    shards: Box<[Mutex<Shard>]>,
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

/// Sorted name table plus the provisional-to-final id mapping.
pub struct SortedNames {
    pub blob: Vec<u8>,
    /// `offsets[i]..offsets[i + 1]` is the byte range of name `i` in `blob`.
    pub offsets: Vec<u32>,
    remap: Vec<u32>,
}

impl SortedNames {
    #[inline]
    pub fn final_id(&self, provisional: u32) -> u32 {
        self.remap[provisional as usize]
    }

    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Interner {
    pub fn new() -> Self {
        Interner { shards: (0..SHARDS).map(|_| Mutex::new(Shard::default())).collect() }
    }

    /// Intern `s`, returning a provisional id that is stable for the lifetime of the interner.
    pub fn intern(&self, s: &[u8]) -> u32 {
        let h = hash_bytes(s);
        // Middle bits pick the shard: hashbrown uses the low bits for the bucket and the top
        // bits for its control bytes, and those must stay well distributed within a shard.
        let shard = ((h >> 24) as usize) & (SHARDS - 1);
        let mut g = self.shards[shard].lock().unwrap();
        let Shard { table, buf, spans } = &mut *g;
        let found = table.find(h, |&i| {
            let (o, l) = spans[i as usize];
            &buf[o as usize..(o + l) as usize] == s
        });
        let idx = match found {
            Some(&i) => i,
            None => {
                let i = spans.len() as u32;
                let off = buf.len() as u32;
                buf.extend_from_slice(s);
                spans.push((off, s.len() as u32));
                let (buf, spans) = (&*buf, &*spans);
                table.insert_unique(h, i, |&j| {
                    let (o, l) = spans[j as usize];
                    hash_bytes(&buf[o as usize..(o + l) as usize])
                });
                i
            }
        };
        (idx << SHARD_BITS) | shard as u32
    }

    /// Sort all names and compute final ids.
    pub fn finalize(self) -> SortedNames {
        let shards: Vec<Shard> = self.shards.into_vec().into_iter().map(|m| m.into_inner().unwrap()).collect();
        let total: usize = shards.iter().map(|s| s.spans.len()).sum();
        let max_len = shards.iter().map(|s| s.spans.len()).max().unwrap_or(0);
        let mut all: Vec<(&[u8], u32)> = Vec::with_capacity(total);
        for (si, sh) in shards.iter().enumerate() {
            for (i, &(o, l)) in sh.spans.iter().enumerate() {
                all.push((&sh.buf[o as usize..(o + l) as usize], ((i as u32) << SHARD_BITS) | si as u32));
            }
        }
        all.par_sort_unstable_by(|a, b| a.0.cmp(b.0));
        let bytes: usize = all.iter().map(|(s, _)| s.len()).sum();
        let mut blob = Vec::with_capacity(bytes);
        let mut offsets = Vec::with_capacity(total + 1);
        let mut remap = vec![u32::MAX; max_len << SHARD_BITS];
        for (rank, (s, prov)) in all.iter().enumerate() {
            offsets.push(blob.len() as u32);
            blob.extend_from_slice(s);
            remap[*prov as usize] = rank as u32;
        }
        offsets.push(blob.len() as u32);
        SortedNames { blob, offsets, remap }
    }
}
