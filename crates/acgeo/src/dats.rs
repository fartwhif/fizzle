//! Retail DAT container reader: header @ 0x140, B-tree directory walk,
//! binary-search `find`, sector-chained record `read`; `find_dat_dir`
//! resolves the explicit dir -> `$AC_DAT_DIR` -> standard locations.
//!
//! Format source of truth: the server C# loader (ACE.DatLoader) / the retail
//! client's DiskController:
//!
//! - offset 0x140: DatDatabaseHeader (u32s, LE): FileType, BlockSize,
//!   FileSize, DataSet, DataSubset, FreeHead, FreeTail, FreeCount, BTree,
//!   NewLRU, OldLRU, UseLRU, MasterMapID, EnginePackVersion,
//!   GamePackVersion, VersionMajor[16], VersionMinor.
//! - BTree: a B-tree directory. Each node is 62 x u32 branches + u32
//!   entryCount + entryCount x DatFile, where DatFile is 6 x u32:
//!   BitFlags, ObjectId, FileOffset, FileSize, Date, Iteration. Branch i
//!   (0..=entryCount) points at a child node (0 = none); branch 0 is 0 on a
//!   leaf (leaf branch slots may hold stale data, so only internal nodes
//!   are descended into).
//! - A record's bytes live in a chain of sectors starting at FileOffset:
//!   each sector is u32 next_sector (0 = last) + (BlockSize-4) data bytes;
//!   the last sector's data is the remaining FileSize bytes. The
//!   concatenated data is the record, which begins with its own u32 Id.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;

/// Fixed size of a directory node: 62*u32 branches + u32 count +
/// 61*24B DatFile (DatDirectoryHeader.ObjectSize).
const DIR_NODE_SIZE: usize = 62 * 4 + 4 + 61 * 24;
/// The DAT magic (FileType, LE "BT").
const DAT_MAGIC: u32 = 0x5442;

#[inline]
fn le_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// The five header fields we keep.
#[derive(Clone, Copy, Debug, Default)]
pub struct DatHeader {
    pub file_type: u32,
    pub block_size: u32,
    pub file_size: u32,
    /// 1 = portal, 2 = cell, 3 = language, 4 = highres.
    pub data_set: u32,
    pub btree: u32,
}

/// A DAT database: directory walked once at open, random record access.
pub struct DatDatabase {
    file: File,
    header: DatHeader,
    /// Sorted by key (ascending); `find` is a binary search.
    keys: Vec<u32>,
    offsets: Vec<u32>,
    sizes: Vec<u32>,
}

impl DatDatabase {
    /// Open the file, read the header at 0x140, and walk the B-tree
    /// directory (one-time; a second or two on the ~805k-entry cell dat).
    pub fn open(path: impl AsRef<std::path::Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut db = Self {
            file,
            header: DatHeader::default(),
            keys: Vec::new(),
            offsets: Vec::new(),
            sizes: Vec::new(),
        };
        db.read_header()?;
        db.build_directory()?;
        Ok(db)
    }

    fn read_header(&mut self) -> io::Result<()> {
        let mut buf = [0u8; 40];
        self.file.seek(SeekFrom::Start(0x140))?;
        self.file.read_exact(&mut buf)?;
        self.header.file_type = le_u32(&buf, 0);
        self.header.block_size = le_u32(&buf, 4);
        self.header.file_size = le_u32(&buf, 8);
        self.header.data_set = le_u32(&buf, 12);
        self.header.btree = le_u32(&buf, 32);
        if self.header.file_type != DAT_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("not a DAT file (bad magic 0x{:x})", self.header.file_type),
            ));
        }
        Ok(())
    }

    fn build_directory(&mut self) -> io::Result<()> {
        let mut keys: Vec<u32> = Vec::new();
        let mut offsets: Vec<u32> = Vec::new();
        let mut sizes: Vec<u32> = Vec::new();
        let mut stack = vec![self.header.btree];
        while let Some(at) = stack.pop() {
            // A directory node is itself a sector chain of fixed
            // ObjectSize, not a contiguous block.
            let node = self.read_chain(at, DIR_NODE_SIZE)?;
            let count = le_u32(&node, 62 * 4) as usize;
            // A node holds at most 61 entries (62 branches); more means a
            // corrupt directory (the entry walk would read past the node).
            if count > 61 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("DAT directory node claims {count} entries (max 61)"),
                ));
            }
            for i in 0..count {
                let e = (62 + 1) * 4 + i * 24;
                // BitFlags(4) ObjectId FileOffset FileSize Date Iteration
                keys.push(le_u32(&node, e + 4));
                offsets.push(le_u32(&node, e + 8));
                sizes.push(le_u32(&node, e + 12));
            }
            // Only internal nodes (Branches[0] != 0) have valid child
            // branches; leaf branch slots may hold stale data.
            if le_u32(&node, 0) != 0 {
                for i in 0..=count {
                    let br = le_u32(&node, i * 4);
                    if br != 0 {
                        stack.push(br);
                    }
                }
            }
        }
        // Sort by key (the walk order is not guaranteed) so lookups are
        // binary searches.
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by_key(|&i| keys[i]);
        let mut sorted_keys = Vec::with_capacity(keys.len());
        let mut sorted_offsets = Vec::with_capacity(keys.len());
        let mut sorted_sizes = Vec::with_capacity(keys.len());
        for i in order {
            sorted_keys.push(keys[i]);
            sorted_offsets.push(offsets[i]);
            sorted_sizes.push(sizes[i]);
        }
        self.keys = sorted_keys;
        self.offsets = sorted_offsets;
        self.sizes = sorted_sizes;
        Ok(())
    }

    /// Number of directory entries.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.keys.len()
    }

    /// All record ObjectIds, ascending.
    #[must_use]
    pub fn ids(&self) -> &[u32] {
        &self.keys
    }

    /// Binary search for an ObjectId: `(FileOffset, FileSize)`.
    #[must_use]
    pub fn find(&self, id: u32) -> Option<(u32, u32)> {
        // Lower bound: the first key >= id, then an exact match.
        let mut lo = 0usize;
        let mut hi = self.keys.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.keys[mid] < id {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo < self.keys.len() && self.keys[lo] == id {
            return Some((self.offsets[lo], self.sizes[lo]));
        }
        None
    }

    /// Read a record's bytes (the sector chain concatenated).
    pub fn read(&mut self, id: u32) -> Option<Vec<u8>> {
        let (offset, size) = self.find(id)?;
        self.read_chain(offset, size as usize).ok()
    }

    /// Concatenate `size` bytes from the sector chain starting at `offset`.
    /// Each sector is u32 next_sector (0 = last) + (BlockSize-4) data bytes;
    /// the last sector's data is the remaining bytes.
    fn read_chain(&mut self, offset: u32, size: usize) -> io::Result<Vec<u8>> {
        let mut out = vec![0u8; size];
        let block = self.header.block_size as usize;
        let mut pos = offset;
        let mut got = 0usize;
        let mut head = [0u8; 4];
        loop {
            self.file.seek(SeekFrom::Start(pos as u64))?;
            self.file.read_exact(&mut head)?;
            let next = u32::from_le_bytes(head);
            if next == 0 {
                self.file.seek(SeekFrom::Start(pos as u64 + 4))?;
                self.file.read_exact(&mut out[got..])?;
                return Ok(out);
            }
            let len = block.saturating_sub(4).min(size - got);
            self.file.seek(SeekFrom::Start(pos as u64 + 4))?;
            self.file.read_exact(&mut out[got..got + len])?;
            got += len;
            pos = next;
            if got >= size {
                return Ok(out);
            }
        }
    }
}

/// Resolve the directory holding the retail dats.
///
/// Order: explicit dir, `$AC_DAT_DIR`, then well-known locations. Returns
/// the first directory containing both `client_portal.dat` and
/// `client_cell_1.dat`, or `None`.
#[must_use]
pub fn find_dat_dir(explicit: Option<&str>) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(e) = explicit {
        if !e.is_empty() {
            candidates.push(e.to_string());
        }
    }
    if let Ok(env_dir) = std::env::var("AC_DAT_DIR") {
        if !env_dir.is_empty() {
            candidates.push(env_dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            candidates.push(format!("{home}/projects/acclient/server/Dats"));
        }
    }
    candidates.push("/ace/Dats".to_string());
    for dir in candidates {
        let p = if let Some(rest) = dir.strip_prefix("~/") {
            format!("{}/{}", std::env::var("HOME").unwrap_or_default(), rest)
        } else {
            dir
        };
        let pb = PathBuf::from(&p);
        if pb.join("client_portal.dat").is_file() && pb.join("client_cell_1.dat").is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_dat_dir_explicit_missing() {
        // A missing explicit dir is never returned; a fallback candidate
        // ($AC_DAT_DIR or a well-known location) may exist on this machine.
        let found = find_dat_dir(Some("/definitely/not/a/dat/dir"));
        if let Some(dir) = found {
            assert_ne!(dir, "/definitely/not/a/dat/dir");
            assert!(PathBuf::from(&dir).join("client_portal.dat").is_file());
            assert!(PathBuf::from(&dir).join("client_cell_1.dat").is_file());
        }
    }

    /// A single-directory-node DAT file: header @ 0x140, the node in the
    /// 0x400 sector (branches zero = leaf), one 0x800 sector per record.
    fn minimal_dat(count: u32, records: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut f = vec![0u8; 0x1400 + records.len() * 0x800];
        let u32 = |v: u32| v.to_le_bytes();
        f[0x140..0x144].copy_from_slice(&u32(0x5442)); // FileType "BT"
        f[0x144..0x148].copy_from_slice(&u32(4096)); // BlockSize
        f[0x148..0x14c].copy_from_slice(&u32((0x1400 + records.len() * 0x800) as u32)); // FileSize
        f[0x14c..0x150].copy_from_slice(&u32(2)); // DataSet = cell
        f[0x160..0x164].copy_from_slice(&u32(0x400)); // BTree
        // Node sector: u32 next = 0, then 62 branch slots + count + entries.
        let node = 0x400 + 4;
        f[node + 62 * 4..node + 62 * 4 + 4].copy_from_slice(&u32(count));
        for (i, (id, rec)) in records.iter().enumerate() {
            let e = node + 252 + i * 24;
            f[e..e + 4].copy_from_slice(&u32(1)); // BitFlags
            f[e + 4..e + 8].copy_from_slice(&u32(*id)); // ObjectId
            f[e + 8..e + 12].copy_from_slice(&u32(0x1400 + i as u32 * 0x800));
            f[e + 12..e + 16].copy_from_slice(&u32(rec.len() as u32));
            f[e + 20..e + 24].copy_from_slice(&u32(1)); // Iteration
            // Record sector: u32 next = 0, then the record bytes.
            let s = 0x1400 + i * 0x800;
            f[s + 4..s + 4 + rec.len()].copy_from_slice(rec);
        }
        f
    }

    fn write_tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// A directory node claiming more than its 61 entry slots is corrupt
    /// (regression: the entry walk read past the 1716-byte node and
    /// panicked on the index).
    #[test]
    fn open_rejects_directory_node_count_over_61() {
        let p = write_tmp("acgeo-dats-count", &minimal_dat(62, &[]));
        let res = DatDatabase::open(&p);
        let _ = std::fs::remove_file(&p);
        assert!(res.is_err(), "corrupt node count must be an error");
    }
}
