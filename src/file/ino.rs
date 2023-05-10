use serde::{Deserialize, Serialize};

const SPLIT: u64 = 32;
const ROOT_INO: u64 = 1;

// inode
// 64 bit |00000000000000000000000000000000|00000000000000000000000000000000|
// first 32 bits are for file inode number giving us +- 4 billion files per tag
// next 32 bits are for tag inode number giving us +- 4 billion tags
// use the file bits to check if it's a file or a tag (file bits == 0 -> tag)
// this allows us to assign unique inodes to files across folders and still identify them easily.

// this solves issues that file managers may have with recurring inode numbers.

#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Ino(pub u64);

pub type FileNumber = u64;
pub type TagNumber = u64;

impl Ino {
    pub fn is_tag(&self) -> bool {
        self.file() == 0
    }

    pub fn is_file(&self) -> bool {
        !self.is_tag()
    }

    pub fn file(&self) -> FileNumber {
        self.0 >> SPLIT
    }

    pub fn tag(&self) -> TagNumber {
        self.0 & (!0 >> SPLIT)
    }

    /// Construct [Ino] from a file and tag part, both unshifted.
    /// If the file part is shifted, just use the Ino constructor.
    pub fn from_parts(file: u64, tag: u64) -> Ino {
        Ino((file << SPLIT) | tag)
    }

    pub fn from_tag(tag: u64) -> Ino {
        Ino::from_parts(0, tag)
    }

    pub const ROOT: Ino = Ino(ROOT_INO);
}
