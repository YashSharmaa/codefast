//! Segment writer: turns extracted facts into the on-disk format.

use super::format::*;
use crate::facts::{FileOut, NONE};
use crate::interner::SortedNames;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::Path;

#[derive(Default, Debug, Clone)]
pub struct SegmentStats {
    pub files: usize,
    pub parsed_files: usize,
    pub defs: usize,
    pub refs: usize,
    pub imports: usize,
    pub names: usize,
    pub postings: usize,
    pub bytes: u64,
    pub lines: u64,
}

struct Encoded {
    refs: Vec<u8>,
    imports: Vec<u8>,
    /// Distinct final name ids mentioned by this file, ascending, with occurrence counts.
    words: Vec<(u32, u8)>,
}

/// Write a segment. `files` must be sorted by path; name ids inside are provisional ids from the
/// interner that produced `names`.
pub fn write_segment(path: &Path, root: &str, files: &[FileOut], names: &SortedNames, extra_meta: &[(&str, String)]) -> Result<SegmentStats> {
    let (mut stats, bytes) = encode_segment(root, files, names, extra_meta, &[])?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    stats.bytes = bytes.len() as u64;
    Ok(stats)
}

/// Encode a segment into memory. `extra_sections` are appended verbatim (tag, bytes).
pub fn encode_segment(
    root: &str,
    files: &[FileOut],
    names: &SortedNames,
    extra_meta: &[(&str, String)],
    extra_sections: &[(&[u8; 4], &[u8])],
) -> Result<(SegmentStats, Vec<u8>)> {
    let nfiles = files.len();
    let nnames = names.len();
    let fid = |p: u32| -> u32 { if p == NONE { NONE } else { names.final_id(p) } };

    // Paths.
    let mut pstr = Vec::with_capacity(files.iter().map(|f| f.path.len()).sum());
    let mut poff: Vec<u32> = Vec::with_capacity(nfiles + 1);
    for f in files {
        poff.push(pstr.len() as u32);
        pstr.extend_from_slice(f.path.as_bytes());
    }
    poff.push(pstr.len() as u32);

    // Definitions, grouped by file in extraction order (parent / encl indices rely on it).
    let ndefs: usize = files.iter().map(|f| f.defs.len()).sum();
    let mut defs: Vec<DefRec> = Vec::with_capacity(ndefs);
    let mut def_starts: Vec<u32> = Vec::with_capacity(nfiles);
    for f in files {
        let base = defs.len() as u32;
        def_starts.push(base);
        for d in &f.defs {
            let parent = if d.parent != NONE {
                base + d.parent
            } else if d.qual != NONE {
                PARENT_IS_NAME | fid(d.qual)
            } else {
                NONE
            };
            defs.push(DefRec {
                name: fid(d.name),
                parent,
                line: d.line,
                end_line: d.end_line,
                sig_off: d.sig_off,
                type_name: fid(d.type_name),
                sig_len: d.sig_len,
                kind: d.kind as u8,
                flags: d.flags,
                arity_min: d.arity_min,
                arity_max: d.arity_max,
                _pad: 0,
            });
        }
    }

    // Def ids sorted by name.
    let mut dnam: Vec<u32> = (0..ndefs as u32).collect();
    dnam.par_sort_unstable_by_key(|&i| (defs[i as usize].name, i));

    // Reference and import blocks, plus each file's word list.
    let encoded: Vec<Encoded> = files
        .par_iter()
        .map(|f| {
            let mut recs: Vec<RefRec> = f
                .refs
                .iter()
                .map(|r| RefRec { name: fid(r.name), qual: fid(r.qual), line: r.line, encl: r.encl, kind: r.kind as u8, nargs: r.nargs })
                .collect();
            recs.sort_unstable_by_key(|r| (r.name, r.line, r.kind));
            let mut refs = Vec::new();
            if !recs.is_empty() {
                refs.reserve(recs.len() * 5);
                encode_refs(&recs, &mut refs);
            }
            let mut imports = Vec::new();
            if !f.imports.is_empty() {
                let ims: Vec<ImportRec> = f
                    .imports
                    .iter()
                    .map(|im| ImportRec { target: fid(im.target), name: fid(im.name), alias: fid(im.alias), line: im.line, flags: im.flags })
                    .collect();
                encode_imports(&ims, &mut imports);
            }
            // Words: the file's text, plus names that only exist structurally (derived names).
            let mut words: Vec<(u32, u8)> = Vec::with_capacity(f.words.len() + 8);
            for &(wid, tf) in &f.words {
                words.push((fid(wid), tf));
            }
            let mut last = NONE;
            for r in &recs {
                if r.name != last {
                    words.push((r.name, 0));
                    last = r.name;
                }
            }
            for d in &f.defs {
                words.push((fid(d.name), 0));
            }
            words.sort_unstable();
            // Keep one entry per name: the highest count (at least 1).
            let mut w = 0usize;
            for i in 0..words.len() {
                if w > 0 && words[w - 1].0 == words[i].0 {
                    words[w - 1].1 = words[w - 1].1.max(words[i].1);
                } else {
                    words[w] = words[i];
                    w += 1;
                }
            }
            words.truncate(w);
            for e in words.iter_mut() {
                e.1 = e.1.max(1);
            }
            Encoded { refs, imports, words }
        })
        .collect();

    // File records and block blobs.
    let mut frecs: Vec<FileRec> = Vec::with_capacity(nfiles);
    let rblk_len: usize = encoded.iter().map(|e| e.refs.len()).sum();
    let iblk_len: usize = encoded.iter().map(|e| e.imports.len()).sum();
    let mut rblk: Vec<u8> = Vec::with_capacity(rblk_len);
    let mut iblk: Vec<u8> = Vec::with_capacity(iblk_len);
    let mut stats = SegmentStats { files: nfiles, defs: ndefs, names: nnames, ..Default::default() };
    for (i, (f, e)) in files.iter().zip(&encoded).enumerate() {
        frecs.push(FileRec {
            size: f.size,
            mtime_ns: f.mtime_ns,
            hash: f.hash,
            ref_off: rblk.len() as u64,
            imp_off: iblk.len() as u64,
            ref_len: e.refs.len() as u32,
            imp_len: e.imports.len() as u32,
            def_start: def_starts[i],
            def_count: f.defs.len() as u32,
            nlines: f.nlines,
            parse_errors: f.parse_errors,
            lang: f.lang,
            flags: f.flags,
            _pad: [0; 6],
        });
        rblk.extend_from_slice(&e.refs);
        iblk.extend_from_slice(&e.imports);
        stats.refs += f.refs.len();
        stats.imports += f.imports.len();
        stats.lines += f.nlines as u64;
        if f.flags & crate::facts::fflag::UNPARSED == 0 {
            stats.parsed_files += 1;
        }
    }

    // Postings: name -> files mentioning it. Files are visited in ascending id order, so a
    // counting sort leaves every list sorted.
    let mut counts: Vec<u32> = vec![0; nnames + 1];
    for e in &encoded {
        for &(w, _) in &e.words {
            counts[w as usize + 1] += 1;
        }
    }
    for i in 0..nnames {
        counts[i + 1] += counts[i];
    }
    let total = counts[nnames] as usize;
    stats.postings = total;
    let mut flat: Vec<u32> = vec![0; total];
    let mut flat_tf: Vec<u8> = vec![0; total];
    let mut cursor: Vec<u32> = counts[..nnames].to_vec();
    for (file, e) in encoded.iter().enumerate() {
        for &(w, tf) in &e.words {
            let c = &mut cursor[w as usize];
            flat[*c as usize] = file as u32;
            flat_tf[*c as usize] = tf;
            *c += 1;
        }
    }
    drop(cursor);
    drop(encoded);
    let mut wdat: Vec<u8> = Vec::with_capacity(total + total / 2);
    let mut woff: Vec<u32> = Vec::with_capacity(nnames + 1);
    for n in 0..nnames {
        woff.push(wdat.len() as u32);
        let (a, b) = (counts[n] as usize, counts[n + 1] as usize);
        let mut prev = 0u32;
        for i in a..b {
            let f = flat[i];
            put_posting(&mut wdat, if i == a { f } else { f - prev }, flat_tf[i]);
            prev = f;
        }
    }
    woff.push(wdat.len() as u32);
    drop(flat);
    drop(flat_tf);

    let mut meta = String::new();
    meta.push_str(&format!("version={VERSION}\nroot={root}\nfiles={nfiles}\ndefs={ndefs}\nrefs={}\nnames={nnames}\nlines={}\n", stats.refs, stats.lines));
    for (k, v) in extra_meta {
        meta.push_str(&format!("{k}={v}\n"));
    }

    let mut sections: Vec<(&[u8; 4], &[u8])> = vec![
        (b"META", meta.as_bytes()),
        (b"NSTR", &names.blob),
        (b"NOFF", bytemuck::cast_slice(&names.offsets)),
        (b"PSTR", &pstr),
        (b"POFF", bytemuck::cast_slice(&poff)),
        (b"FILE", bytemuck::cast_slice(&frecs)),
        (b"DEFS", bytemuck::cast_slice(&defs)),
        (b"DNAM", bytemuck::cast_slice(&dnam)),
        (b"RBLK", &rblk),
        (b"IBLK", &iblk),
        (b"WOFF", bytemuck::cast_slice(&woff)),
        (b"WDAT", &wdat),
    ];
    sections.extend_from_slice(extra_sections);
    let bytes = layout_sections(&sections);
    stats.bytes = bytes.len() as u64;
    Ok((stats, bytes))
}

fn align8(n: u64) -> u64 {
    (n + 7) & !7
}

/// Lay the sections out behind a header and section table, each 8-byte aligned.
fn layout_sections(sections: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let header_len = std::mem::size_of::<Header>() + sections.len() * std::mem::size_of::<SectionEntry>();
    let mut entries: Vec<SectionEntry> = Vec::with_capacity(sections.len());
    let mut off = align8(header_len as u64);
    for (tag, data) in sections {
        entries.push(SectionEntry { tag: **tag, _pad: 0, offset: off, len: data.len() as u64 });
        off = align8(off + data.len() as u64);
    }
    let mut out: Vec<u8> = Vec::with_capacity(off as usize);
    let header = Header { magic: MAGIC, version: VERSION, nsections: sections.len() as u32 };
    out.extend_from_slice(bytemuck::bytes_of(&header));
    out.extend_from_slice(bytemuck::cast_slice(&entries));
    for ((_, data), e) in sections.iter().zip(&entries) {
        out.resize(e.offset as usize, 0);
        out.extend_from_slice(data);
    }
    out.resize(off as usize, 0);
    out
}
