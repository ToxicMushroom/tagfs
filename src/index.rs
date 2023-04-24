
use std::rc::Rc;

new_key_type! {
    // #[derive(Serialize, Deserialize)]
    pub struct FileKey;
}
// impl bincode::Encode for FileKey {
//     fn encode<E: bincode::enc::Encoder>(
//         &self,
//         encoder: &mut E,
//     ) -> Result<(), bincode::error::EncodeError> {
//         Encode::encode(&self.0.as_ffi(), encoder)?;
//         Ok(())
//     }
// }
//
// impl bincode::Decode for FileKey {
//     fn decode<D: Decoder>(
//         decoder: &mut D,
//     ) -> Result<Self, DecodeError> {
//         Ok(Self {
//             0: KeyData::from_ffi(Decode::decode(decoder)?),
//         })
//     }
// }
// impl<'de> bincode::BorrowDecode<'de> for FileKey {
//     fn borrow_decode<D: bincode::de::BorrowDecoder<'de>>(
//         decoder: &mut D,
//     ) -> Result<Self, DecodeError> {
//         Ok(Self {
//             0: KeyData::from_ffi(bincode::BorrowDecode::borrow_decode(decoder)?)
//         })
//     }
// }
use slotmap::__impl::{Deserialize, Serialize};
use slotmap::{new_key_type, SecondaryMap, SlotMap};

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
    pub tags: Vec<(Tag, Rc<TagFiles>)>,
}

#[derive(Serialize, Deserialize)]
pub struct PersistentState {
    pub files: AllFiles,
    pub tags: Tags,
}

impl Tags {
    pub fn sort(&mut self) {
        self.tags.sort_by_key(|(_, files)| files.len())
    }
}
