use std::rc::Rc;

use slotmap::{new_key_type, SecondaryMap, SlotMap};

new_key_type! {
    struct FileKey;
}

pub type AllFiles = SlotMap<FileKey, File>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Tag(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct File {
    pub fsize: u64,
    pub name: String,
    pub inode: u64,
}

impl Tag {
    fn new(tag: String) -> Self {
        Self(tag)
    }
}

// String -> Tag
impl From<String> for Tag {
    fn from(tag: String) -> Self {
        Self::new(tag)
    }
}

type TagFiles = SecondaryMap<FileKey, ()>;

pub struct Tags {
    pub tags: Vec<(Tag, Rc<TagFiles>)>
}

impl Tags {
    pub fn sort(&mut self) {
        self.tags.sort_by_key(|(_, files)| files.len())
    }
}

