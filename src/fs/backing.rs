use std::cell::RefCell;
use std::cmp::min;
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::os::unix::prelude::*;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use fuser::{FileAttr, FileType};

use crate::fs::FileHandle;

pub trait BackingFS {
    fn get_metadata<P: AsRef<Path>>(&self, path: P) -> Result<FileAttr, Self::Error>;
    fn open<P: AsRef<Path>>(&self, path: P) -> Result<FileHandle, Self::Error>;
    fn create<P: AsRef<Path>>(&self, path: P) -> Result<FileHandle, Self::Error>;
    fn read(&self, handle: FileHandle, offset: u64, size: u64) -> Result<Vec<u8>, Self::Error>;
    fn write(&self, handle: FileHandle, data: &[u8]) -> Result<(), Self::Error>;
    fn release(&self, handle: FileHandle);

    type Error;
}

#[derive(Debug)]
pub struct ExternalFS {
    source_path: PathBuf,
    open_files: RefCell<HashMap<FileHandle, File>>,
}

impl ExternalFS {
    fn relative_path<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.source_path.join(path)
    }

    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            open_files: RefCell::new(HashMap::new()),
            source_path: path.as_ref().to_path_buf(),
        }
    }

    pub fn source_path(&self) -> &Path {
        self.source_path.as_path()
    }
}

impl BackingFS for ExternalFS {
    fn get_metadata<P: AsRef<Path>>(&self, path: P) -> Result<FileAttr, Self::Error> {
        fs::metadata(self.relative_path(path)).map(|md| {
            let ctime = md.created().unwrap_or(UNIX_EPOCH);

            FileAttr {
                ino: md.ino(),
                size: md.size(),
                blocks: md.blocks(),
                atime: md.accessed().unwrap_or(UNIX_EPOCH), // 1970-01-01 00:00:00
                mtime: md.modified().unwrap_or(UNIX_EPOCH),
                ctime,
                crtime: ctime,
                kind: FileType::RegularFile,
                perm: md.permissions().mode() as u16,
                nlink: md.nlink() as u32,
                uid: md.uid(),
                gid: md.gid(),
                rdev: md.rdev() as u32,
                flags: 0,
                blksize: md.blksize() as u32,
            }
        })
    }

    fn open<P: AsRef<Path>>(&self, path: P) -> Result<FileHandle, Self::Error> {
        let fh = File::open(self.relative_path(path))?;

        let handle = FileHandle(fh.as_raw_fd() as u64);

        self.open_files.borrow_mut().insert(handle, fh);

        Ok(handle)
    }

    fn create<P: AsRef<Path>>(&self, path: P) -> Result<FileHandle, Self::Error> {
        let fh = File::create(self.relative_path(path))?;

        let handle = FileHandle(fh.as_raw_fd() as u64);

        self.open_files.borrow_mut().insert(handle, fh);

        Ok(handle)
    }

    fn read(&self, handle: FileHandle, offset: u64, size: u64) -> Result<Vec<u8>, Self::Error> {
        let file = &self.open_files.borrow_mut()[&handle];
        let file_size = file.metadata()?.len();

        let size = min(size, file_size.saturating_sub(offset));

        let mut buf = vec![0; size as usize];
        file.read_exact_at(&mut buf, offset)?;

        Ok(buf)
    }

    fn write(&self, handle: FileHandle, data: &[u8]) -> Result<(), Self::Error> {
        let mut files = self.open_files.borrow_mut();
        let file = files.get_mut(&handle).unwrap();
        file.write_all(data)?;

        Ok(())
    }

    fn release(&self, handle: FileHandle) {
        self.open_files.borrow_mut().remove(&handle);
    }

    type Error = std::io::Error;
}
