use serde::{Deserialize, Serialize};

pub mod backing;
pub mod tag;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
pub struct FileHandle(pub u64);
