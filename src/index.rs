use std::cell::RefCell;
use std::rc::Rc;

use slotmap::{new_key_type, SecondaryMap, SlotMap};
use slotmap::__impl::{Deserialize, Serialize};

new_key_type! {
    pub struct FileKey;
}
pub type AllFiles = SlotMap<FileKey, File>;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tag(pub(crate) String);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
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

// String -> Tag
impl From<&str> for Tag {
    fn from(tag: &str) -> Self {
        Self::new(tag.to_string())
    }
}

// this is a set
pub(crate) type TagFiles = SecondaryMap<FileKey, ()>;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Tags {
    pub tags: Vec<(Tag, Rc<RefCell<TagFiles>>)>,
}

#[derive(Serialize, Deserialize)]
pub struct PersistentState {
    pub files: AllFiles,
    pub tags: Tags,
}

impl Tags {
    pub fn sort(&mut self) {
        self.tags.sort_by_key(|(_, files)| files.borrow().len())
    }
}
