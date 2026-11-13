//! Memory-mapped segment reader. Opening a segment costs one `mmap`; every accessor reads the
//! mapped bytes in place, so a cold CLI invocation answers in milliseconds even on huge indexes.

use super::format::*;
use anyhow::{Context, Result, bail};
use memmap2::Mmap;
use std::fs::File;
use std::path::Path;

#[derive(Clone, Copy, Default)]
struct Sec {
    off: usize,
    len: usize,
}

enum Backing {
    Map(Mmap),
    Mem(Vec<u8>),
}

pub struct Segment {
    data: Backing,
    meta: Sec,
    nstr: Sec,
    noff: Sec,
    pstr: Sec,
    poff: Sec,
    file: Sec,
    defs: Sec,
    dnam: Sec,
    rblk: Sec,
    iblk: Sec,
    woff: Sec,
    wdat: Sec,
    table: Vec<([u8; 4], Sec)>,
}

impl Segment {
    pub fn open(path: &Path) -> Result<Segment> {
        let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the file is only ever replaced by atomic rename, never modified in place.
        let map = unsafe { Mmap::map(&f) }.with_context(|| format!("mmap {}", path.display()))?;
        Self::from_backing(Backing::Map(map))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Segment> {
        Self::from_backing(Backing::Mem(bytes))
    }

    fn from_backing(data: Backing) -> Result<Segment> {
        let mut seg = Segment {
            data,
            meta: Sec::default(),
            nstr: Sec::default(),
            noff: Sec::default(),
            pstr: Sec::default(),
            poff: Sec::default(),
            file: Sec::default(),
            defs: Sec::default(),
            dnam: Sec::default(),
            rblk: Sec::default(),
            iblk: Sec::default(),
            woff: Sec::default(),
            wdat: Sec::default(),
            table: Vec::new(),
        };
        let all = seg.all();
        let hlen = std::mem::size_of::<Header>();
        if all.len() < hlen {
            bail!("index file is truncated");
        }
        let header: Header = bytemuck::pod_read_unaligned(&all[..hlen]);
        if header.magic != MAGIC || header.version != VERSION {
            bail!("index format mismatch (rebuild with `codefast index --force`)");
        }
        let elen = std::mem::size_of::<SectionEntry>();
        let n = header.nsections as usize;
        if all.len() < hlen + n * elen {
            bail!("index file is truncated");
        }
        let total = all.len();
        let mut found: Vec<([u8; 4], Sec)> = Vec::with_capacity(n);
        for i in 0..n {
            let e: SectionEntry = bytemuck::pod_read_unaligned(&all[hlen + i * elen..hlen + (i + 1) * elen]);
            let (off, len) = (e.offset as usize, e.len as usize);
            if off.checked_add(len).is_none_or(|end| end > total) || off % 8 != 0 {
                bail!("index file is corrupt");
            }
            found.push((e.tag, Sec { off, len }));
        }
        let get = |tag: &[u8; 4]| -> Result<Sec> {
            found.iter().find(|(t, _)| t == tag).map(|(_, s)| *s).with_context(|| format!("index is missing section {}", String::from_utf8_lossy(tag)))
        };
        seg.meta = get(b"META")?;
        seg.nstr = get(b"NSTR")?;
        seg.noff = get(b"NOFF")?;
        seg.pstr = get(b"PSTR")?;
        seg.poff = get(b"POFF")?;
        seg.file = get(b"FILE")?;
        seg.defs = get(b"DEFS")?;
        seg.dnam = get(b"DNAM")?;
        seg.rblk = get(b"RBLK")?;
        seg.iblk = get(b"IBLK")?;
        seg.woff = get(b"WOFF")?;
        seg.wdat = get(b"WDAT")?;
        if seg.noff.len < 4 || seg.poff.len < 4 || seg.noff.len % 4 != 0 || seg.poff.len % 4 != 0 {
            bail!("index file is corrupt");
        }
        if seg.file.len % std::mem::size_of::<FileRec>() != 0 || seg.defs.len % std::mem::size_of::<DefRec>() != 0 {
            bail!("index file is corrupt");
        }
        seg.table = found;
        Ok(seg)
    }

    #[inline]
    fn all(&self) -> &[u8] {
        match &self.data {
            Backing::Map(m) => m,
            Backing::Mem(v) => v,
        }
    }

    #[inline]
    fn sec(&self, s: Sec) -> &[u8] {
        &self.all()[s.off..s.off + s.len]
    }

    // ----- meta -----

    pub fn meta(&self, key: &str) -> Option<&str> {
        let text = std::str::from_utf8(self.sec(self.meta)).ok()?;
        text.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
    }

    /// An optional section by tag.
    pub fn section(&self, tag: &[u8; 4]) -> Option<&[u8]> {
        self.table.iter().find(|(t, _)| t == tag).map(|(_, s)| self.sec(*s))
    }

    pub fn byte_len(&self) -> usize {
        self.all().len()
    }

    // ----- names -----

    #[inline]
    fn noffs(&self) -> &[u32] {
        bytemuck::cast_slice(self.sec(self.noff))
    }

    #[inline]
    pub fn n_names(&self) -> u32 {
        (self.noffs().len() - 1) as u32
    }

    #[inline]
    pub fn name(&self, id: u32) -> &[u8] {
        let o = self.noffs();
        let i = id as usize;
        if i + 1 >= o.len() {
            return b"";
        }
        &self.sec(self.nstr)[o[i] as usize..o[i + 1] as usize]
    }

    pub fn name_str(&self, id: u32) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.name(id))
    }

    /// Exact lookup by binary search over the sorted name table.
    pub fn name_id(&self, s: &[u8]) -> Option<u32> {
        let (mut lo, mut hi) = (0u32, self.n_names());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.name(mid).cmp(s) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }

    /// Range of name ids that start with `prefix`.
    pub fn name_prefix_range(&self, prefix: &[u8]) -> (u32, u32) {
        let n = self.n_names();
        let lower = |target: &dyn Fn(&[u8]) -> bool| -> u32 {
            let (mut lo, mut hi) = (0u32, n);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if target(self.name(mid)) { lo = mid + 1 } else { hi = mid }
            }
            lo
        };
        let start = lower(&|s: &[u8]| s < prefix);
        let end = lower(&|s: &[u8]| s < prefix || s.starts_with(prefix));
        (start, end)
    }

    /// The whole name blob and its offsets, for linear scans (substring, regex, fuzzy).
    pub fn names_raw(&self) -> (&[u8], &[u32]) {
        (self.sec(self.nstr), self.noffs())
    }

    // ----- files -----

    #[inline]
    pub fn files(&self) -> &[FileRec] {
        bytemuck::cast_slice(self.sec(self.file))
    }

    #[inline]
    pub fn n_files(&self) -> u32 {
        self.files().len() as u32
    }

    #[inline]
    pub fn path(&self, file: u32) -> &str {
        let o: &[u32] = bytemuck::cast_slice(self.sec(self.poff));
        let i = file as usize;
        if i + 1 >= o.len() {
            return "";
        }
        std::str::from_utf8(&self.sec(self.pstr)[o[i] as usize..o[i + 1] as usize]).unwrap_or("")
    }

    pub fn file_by_path(&self, path: &str) -> Option<u32> {
        let (mut lo, mut hi) = (0u32, self.n_files());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.path(mid).cmp(path) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }

    // ----- definitions -----

    #[inline]
    pub fn defs(&self) -> &[DefRec] {
        bytemuck::cast_slice(self.sec(self.defs))
    }

    #[inline]
    pub fn def(&self, id: u32) -> &DefRec {
        &self.defs()[id as usize]
    }

    /// Definitions of a file and the global id of the first one.
    pub fn defs_of_file(&self, file: u32) -> (u32, &[DefRec]) {
        let f = &self.files()[file as usize];
        let (s, n) = (f.def_start as usize, f.def_count as usize);
        (f.def_start, &self.defs()[s..s + n])
    }

    /// File that contains a definition (definitions are grouped by file).
    pub fn file_of_def(&self, def: u32) -> u32 {
        let files = self.files();
        let i = files.partition_point(|f| f.def_start <= def);
        // `i - 1` is the last file starting at or before `def`; skip back over empty files.
        let mut i = i.saturating_sub(1);
        while i > 0 && files[i].def_count == 0 {
            i -= 1;
        }
        i as u32
    }

    /// Ids of all definitions with this name.
    pub fn defs_named(&self, name: u32) -> &[u32] {
        let dnam: &[u32] = bytemuck::cast_slice(self.sec(self.dnam));
        let defs = self.defs();
        let lo = dnam.partition_point(|&d| defs[d as usize].name < name);
        let hi = lo + dnam[lo..].partition_point(|&d| defs[d as usize].name == name);
        &dnam[lo..hi]
    }

    /// Def ids ordered by name id: lets callers walk "all definitions whose name is in a range".
    pub fn defs_by_name(&self) -> &[u32] {
        bytemuck::cast_slice(self.sec(self.dnam))
    }

    // ----- references, imports, postings -----

    pub fn refs_of_file(&self, file: u32) -> RefIter<'_> {
        let f = &self.files()[file as usize];
        let blk = self.sec(self.rblk);
        let (s, e) = (f.ref_off as usize, f.ref_off as usize + f.ref_len as usize);
        RefIter::new(blk.get(s..e).unwrap_or(&[]))
    }

    pub fn imports_of_file(&self, file: u32) -> ImportIter<'_> {
        let f = &self.files()[file as usize];
        let blk = self.sec(self.iblk);
        let (s, e) = (f.imp_off as usize, f.imp_off as usize + f.imp_len as usize);
        ImportIter::new(blk.get(s..e).unwrap_or(&[]))
    }

    /// Posting list of `name`: the files whose text or facts mention it, ascending, with counts.
    pub fn postings(&self, name: u32) -> PostingIter<'_> {
        let woff: &[u32] = bytemuck::cast_slice(self.sec(self.woff));
        let i = name as usize;
        if i + 1 >= woff.len() {
            return PostingIter::new(&[]);
        }
        let dat = self.sec(self.wdat);
        PostingIter::new(dat.get(woff[i] as usize..woff[i + 1] as usize).unwrap_or(&[]))
    }

    /// Files that mention `name`, ascending.
    pub fn files_with(&self, name: u32) -> impl Iterator<Item = u32> + '_ {
        self.postings(name).map(|(f, _)| f)
    }

    /// Number of bytes in the posting list: a cheap proxy for how widespread a name is.
    pub fn posting_len(&self, name: u32) -> u32 {
        let woff: &[u32] = bytemuck::cast_slice(self.sec(self.woff));
        let i = name as usize;
        if i + 1 >= woff.len() { 0 } else { woff[i + 1] - woff[i] }
    }

    pub fn section_sizes(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("names", self.nstr.len + self.noff.len),
            ("paths", self.pstr.len + self.poff.len),
            ("files", self.file.len),
            ("defs", self.defs.len + self.dnam.len),
            ("refs", self.rblk.len),
            ("imports", self.iblk.len),
            ("postings", self.woff.len + self.wdat.len),
        ]
    }
}
