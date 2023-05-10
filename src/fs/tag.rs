use std::cell::RefCell;
use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt::{Debug, Formatter};
use std::iter;
use std::rc::{Rc, Weak};
use std::time::{Duration, UNIX_EPOCH};

use bimap::BiMap;
use bincode::serde::Compat;
use fuser::FileType::{Directory, RegularFile};
use fuser::{
    FileAttr, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, Request,
};
use indexmap::IndexMap;
use libc::{EIO, ENOENT, ENOTDIR, ENOTSUP};
use log::{debug, error, warn};
use serde::{Deserialize, Serialize};

use crate::file::{FileNumber, Ino, TagNumber};
use crate::fs::backing::BackingFS;
use crate::fs::FileHandle;

const TTL: Duration = Duration::new(0, 0);

macro_rules! err {
    ($reply:expr, $err:expr) => {{
        $reply.error($err);
        return;
    }};
}

type FileName = OsString;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tag(pub OsString);

/// Represents a single folder in the tagfs system,
/// which is some intersection of tags composed by the tag of this node and its parents.
struct TagNode {
    /// The unique *inode* tag part for this node
    ino_part: u64,
    /// The number of the single tag represented by this node, which may be the same as
    /// `ino_part`, but this is usually not the case.
    tag: TagNumber,
    parent: Option<Rc<RefCell<TagNode>>>,
    children: Vec<Rc<RefCell<TagNode>>>,
}

impl TagNode {
    pub fn collect_tags(&self) -> Vec<TagNumber> {
        match &self.parent {
            None => vec![], // The final parent is always the root, which we don't want to include in this list!
            Some(p) => {
                let mut tags = p.borrow().collect_tags();
                tags.push(self.tag);
                tags
            }
        }
    }

    pub fn find_child(&self, tag: TagNumber) -> Option<Rc<RefCell<TagNode>>> {
        self.children
            .iter()
            .find(|tn| tn.borrow().tag == tag)
            .map(|child| child.clone())
    }
}

impl Debug for TagNode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TagNode")
            .field("ino_part", &self.ino_part)
            .field("tag", &self.tag)
            .field("children", &self.children)
            .finish()
    }
}

#[derive(Debug)]
pub struct TagTree {
    root: Rc<RefCell<TagNode>>,
    cache: HashMap<u64, Weak<RefCell<TagNode>>>,
    counter: u64,
}

impl Default for TagTree {
    fn default() -> Self {
        Self::new()
    }
}

impl TagTree {
    fn new() -> Self {
        let root_no = Ino::ROOT.0;
        let root = Rc::new(RefCell::new(TagNode {
            ino_part: root_no,
            tag: root_no,
            children: Vec::new(),
            parent: None,
        }));
        let weak = Rc::downgrade(&root);
        Self {
            root,
            cache: HashMap::from_iter(iter::once((root_no, weak))),
            counter: root_no,
        }
    }

    fn lookup(&self, tag: u64) -> Option<Rc<RefCell<TagNode>>> {
        self.cache.get(&tag).and_then(|w| w.upgrade())
    }

    fn add_to(&mut self, node: Rc<RefCell<TagNode>>, tag: TagNumber) -> Rc<RefCell<TagNode>> {
        // Increase the ino counter by 1
        self.counter += 1;

        // Create the new node, referencing its parent
        let new = Rc::new(RefCell::new(TagNode {
            ino_part: self.counter,
            tag,
            parent: Some(node.clone()),
            children: vec![],
        }));

        let weak = Rc::downgrade(&new);

        // Add the new node to its parent's children
        node.borrow_mut().children.push(new.clone());

        self.cache.insert(self.counter, weak);

        new
    }

    fn add_to_if_needed(
        &mut self,
        node: Rc<RefCell<TagNode>>,
        tag: TagNumber,
    ) -> Rc<RefCell<TagNode>> {
        let child = node.borrow().find_child(tag);
        match child {
            None => self.add_to(node, tag),
            Some(c) => c,
        }
    }

    /// Create a TagNode for an entirely new tag
    fn create_new(&mut self) -> u64 {
        let root = self.root.clone();
        let tnb = self.counter + 1;
        self.add_to(root, tnb);

        tnb
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PersistentState {
    #[serde(with = "indexmap::serde_seq")]
    tag_content: IndexMap<TagNumber, HashSet<FileNumber>>,
    files: BiMap<FileNumber, FileName>,
    tags: BiMap<TagNumber, FileName>,
    file_tally: FileNumber,
}

#[derive(Debug)]
pub struct TagFS<B> {
    backing: B,
    tree: TagTree,
    tag_content: IndexMap<TagNumber, HashSet<FileNumber>>,
    files: BiMap<FileNumber, FileName>,
    tags: BiMap<TagNumber, FileName>,
    file_tally: FileNumber,
}

impl<B> TagFS<B> {
    pub fn new(backing: B) -> TagFS<B> {
        Self {
            backing,
            tree: Default::default(),
            tag_content: Default::default(),
            files: Default::default(),
            tags: Default::default(),
            file_tally: 1,
        }
    }

    pub fn new_from_save(backing: B) -> anyhow::Result<TagFS<B>>
    where
        B: BackingFS,
        <B as BackingFS>::Error: Error + Send + Sync + 'static,
    {
        // Leverage the simple implementation of backingfs to read out the savefile
        let handle = backing.open(".tagfs")?;
        let savefile = backing.read(handle, 0, u64::MAX)?;
        backing.release(handle);

        let (
            Compat(PersistentState {
                tag_content,
                tags,
                files,
                file_tally,
            }),
            _,
        ): (Compat<PersistentState>, _) =
            bincode::decode_from_slice(&savefile, bincode::config::standard())?;

        Ok(TagFS {
            backing,
            tree: Default::default(),
            tag_content,
            files,
            tags,
            file_tally,
        })
    }

    pub fn get_fnb_by_name<N: AsRef<OsStr>>(&self, name: N) -> Option<FileNumber> {
        self.files.get_by_right(name.as_ref()).copied()
    }

    pub fn get_fnm_by_number(&self, number: FileNumber) -> Option<&FileName> {
        self.files.get_by_left(&number)
    }

    pub fn get_tnb_by_name<N: AsRef<OsStr>>(&self, name: N) -> Option<TagNumber> {
        self.tags.get_by_right(name.as_ref()).copied()
    }

    pub fn calculate_intersection(&self, path: &[TagNumber]) -> HashSet<FileNumber> {
        if path.is_empty() {
            return self.files.left_values().copied().collect();
        }

        let sets = self
            .tag_content
            .iter()
            .filter(|(tn, _)| path.contains(tn))
            .map(|(_, set)| set)
            .collect::<Vec<_>>();

        let (start, sets) = sets.split_first().unwrap();
        let mut result = (*start).clone();
        for set in sets {
            result = result.intersection(set).copied().collect()
        }

        result
    }

    pub fn create_tag(&mut self, tag: FileName) -> TagNumber {
        let tnb = self.tree.create_new();

        self.tag_content.insert(tnb, Default::default());
        self.tags.insert(tnb, tag);

        tnb
    }

    pub fn add_file(&mut self, file: FileName) -> FileNumber {
        self.file_tally += 1;
        let fnb = self.file_tally;
        self.files.insert(fnb, file);

        fnb
    }

    pub fn add_file_to(&mut self, file: FileNumber, to: TagNumber) {
        self.tag_content.get_mut(&to).unwrap().insert(file);
    }

    pub fn remove_file_from(&mut self, file: FileNumber, from: TagNumber) {
        self.tag_content.get_mut(&from).unwrap().remove(&file);
    }

    pub fn omit_file(&mut self, fnb: FileNumber) {
        self.files.remove_by_left(&fnb);
        self.tag_content.values_mut().for_each(|v| {
            v.remove(&fnb);
        });
    }
}

impl<B> TagFS<B>
where
    B: BackingFS,
    <B as BackingFS>::Error: Error + Send + Sync + 'static,
{
    /// Re-index the file-system, omitting any files not present in the new index,
    /// but retaining any files that were there before.
    pub fn repopulate(&mut self, files: impl IntoIterator<Item = FileName>) {
        let mut files: HashSet<FileName> = files.into_iter().collect();
        files.remove::<OsStr>(".tagfs".as_ref());

        // Omit old files, and remove files that stay from the `files` set
        self.files.retain(|fnb, fnm| {
            if files.remove(fnm) {
                // Great, this file is retained.
                true
            } else {
                debug!("removing '{}'", fnm.to_string_lossy());

                // This file has to be omitted!
                self.tag_content.values_mut().for_each(|v| {
                    v.remove(&fnb);
                });

                false
            }
        });

        // Everything in `files` is now new: add them as new files
        files.into_iter().for_each(|f| {
            debug!("adding new file '{}'", f.to_string_lossy());

            self.add_file(f);
        });

        if let Err(error) = self.save() {
            error!("failed to save: {error}");
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let vec = bincode::encode_to_vec(
            Compat(PersistentState {
                tag_content: self.tag_content.clone(),
                files: self.files.clone(),
                tags: self.tags.clone(),
                file_tally: self.file_tally,
            }),
            bincode::config::standard(),
        )?;

        let handle = self.backing.create(".tagfs")?;
        self.backing.write(handle, &vec)?;

        Ok(())
    }
}

impl<B: BackingFS> Filesystem for TagFS<B>
where
    <B as BackingFS>::Error: Debug + Error + Send + Sync + 'static,
{
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let parent_ino = Ino(parent);
        if parent_ino.is_file() {
            err!(reply, ENOTDIR);
        }

        // Find the `TagNode` in the tag tree
        let Some(parent) = self.tree.lookup(parent_ino.tag()) else {
            err!(reply, ENOENT);
        };

        let file = match self.get_fnb_by_name(name) {
            Some(file) => file, // Great, it's a file!
            None => {
                // Great, it's not a file, but it might be a tag.
                let Some(tn) = self.tags.get_by_right(name).copied() else {
                    // It's not a file and not a tag; get out!
                    err!(reply, ENOENT)
                };

                let node = self.tree.add_to_if_needed(parent, tn);
                let ino = Ino::from_tag(node.borrow().ino_part);
                reply.entry(&TTL, &create_folder_attrs(ino), 0);
                return;
            }
        };

        let path = parent.borrow().collect_tags();
        // For the lookup to pass, `file` must be present in each of the tags in the path
        if path.into_iter().all(|tag| {
            self.tag_content
                .get(&tag)
                .map(|set| set.contains(&file))
                .unwrap_or(false)
        }) {
            let Ok(mut fa) = self.backing.get_metadata(name) else {
                error!("Failed to get metadata for '{}' from backing fs", name.to_string_lossy());
                err!(reply, EIO);
            };

            fa.ino = Ino::from_parts(file, parent_ino.tag()).0;

            reply.entry(&TTL, &fa, 0);
        } else {
            err!(reply, ENOENT);
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        let ino = Ino(ino);

        if ino.is_tag() {
            reply.attr(&TTL, &create_folder_attrs(ino))
        } else {
            let Some(name) = self.get_fnm_by_number(ino.file()) else { err!(reply, ENOENT) };

            let Ok(mut fa) = self.backing.get_metadata(name) else {
                error!("Failed to get metadata for '{}' from backing fs", name.to_string_lossy());
                err!(reply, EIO);
            };

            fa.ino = ino.0;
            reply.attr(&TTL, &fa);
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        if name == ".Trash-1000" {
            err!(reply, ENOTSUP);
        }
        let tnb = self.create_tag(name.to_os_string());

        reply.entry(&TTL, &create_folder_attrs(Ino::from_tag(tnb)), 0);

        if let Err(error) = self.save() {
            error!("failed to save: {error}");
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let parent = Ino(parent);
        let Some(parent) = self.tree.lookup(parent.tag()) else {
            err!(reply, ENOENT);
        };

        // should we check if the file actually even exists under this tag?
        // the operation will succeed without, but do nothing.
        let Some(file) = self.get_fnb_by_name(name) else { err!(reply, ENOENT); };

        let tags = parent.borrow().collect_tags();
        for tag in tags {
            self.remove_file_from(file, tag);
        }

        reply.ok();

        if let Err(error) = self.save() {
            error!("failed to save: {error}");
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        // If we're renaming a tag, the parent(s) don't matter
        if let Some(tag) = self.get_tnb_by_name(name) {
            self.tags.insert(tag, newname.to_os_string());
            reply.ok();

            if let Err(error) = self.save() {
                error!("failed to save: {error}");
            }

            return;
        }

        // If we're moving a file, disallow renames
        if name == newname && parent != newparent {
            let Some(file) = self.get_fnb_by_name(name) else { err!(reply, ENOENT); };

            let parent = Ino(parent);
            let Some(parent) = self.tree.lookup(parent.tag()) else {
                err!(reply, ENOENT);
            };
            let newparent = Ino(newparent);
            let Some(newparent) = self.tree.lookup(newparent.tag()) else {
                err!(reply, ENOENT);
            };

            let oldtags = parent.borrow().collect_tags();
            let newtags = newparent.borrow().collect_tags();

            for tag in oldtags {
                self.remove_file_from(file, tag);
            }
            for tag in newtags {
                self.add_file_to(file, tag);
            }

            reply.ok();

            if let Err(error) = self.save() {
                error!("failed to save: {error}");
            }

            return;
        }

        reply.error(ENOTSUP);
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        let ino = Ino(ino);
        if !ino.is_file() {
            err!(reply, ENOENT)
        }

        let Some(filename) = self.get_fnm_by_number(ino.file()) else {
            err!(reply, ENOENT)
        };

        match self.backing.open(filename) {
            Ok(fh) => {
                reply.opened(fh.0, 0);
            }
            Err(e) => {
                error!(
                    "failed to open file '{}' from backing: {e:?}",
                    filename.to_string_lossy()
                );
                reply.error(EIO);
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let result = self
            .backing
            .read(FileHandle(fh), offset as u64, size as u64);

        match result {
            Ok(buf) => {
                reply.data(&buf);
            }
            Err(e) => {
                warn!("read failed because of backing error: {e:?}");

                reply.error(EIO);
            }
        }
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.backing.release(FileHandle(fh));

        reply.ok();
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let mut offset = offset as usize;
        let ino = Ino(ino);
        if ino.is_file() {
            err!(reply, ENOTDIR)
        }

        // Find the `TagNode` in the tag tree
        let Some(dir) = self.tree.lookup(ino.tag()) else {
            err!(reply, ENOENT);
        };

        'full: {
            // The `offset` counter, accounting for the . and .. entries
            let mut idx = offset as i64 + 3;

            // Skip . and .. or not?
            if offset >= 2 {
                offset -= 2;
            } else {
                if reply.add(ino.0, 1, Directory, ".") {
                    break 'full;
                }
                if reply.add(Ino::from_tag(dir.borrow().ino_part).0, 2, Directory, "..") {
                    break 'full;
                }
            }

            // Dirs to list
            let used_tags = dir.borrow().collect_tags();
            let tags = self.tags.clone();

            // Only keep tags that aren't present in the current dir's tag list
            let mut tags = tags
                .into_iter()
                .filter(|(l, _)| !used_tags.contains(l))
                .collect::<Vec<_>>();

            let to_drain = min(tags.len(), offset);
            tags.drain(0..to_drain);
            offset = offset.saturating_sub(to_drain);

            if offset == 0 {
                // Turn those into TagNodes, generating them as required
                for (tag, name) in tags.into_iter().map(|(tn, name)| {
                    (
                        self.tree
                            .add_to_if_needed(dir.clone(), tn)
                            .borrow()
                            .ino_part,
                        name,
                    )
                }) {
                    if reply.add(Ino::from_tag(tag).0, idx, Directory, name) {
                        break 'full;
                    }
                    idx += 1;
                }
            }

            // Files to list
            let mut files = self
                .calculate_intersection(&used_tags)
                .into_iter()
                .collect::<Vec<_>>();

            let to_drain = min(files.len(), offset);
            files.drain(0..to_drain);
            offset = offset.saturating_sub(to_drain);

            if offset == 0 {
                for file in files {
                    let filename = self.get_fnm_by_number(file).expect("file without a name");
                    if reply.add(
                        Ino::from_parts(file, ino.tag()).0,
                        idx,
                        RegularFile,
                        filename,
                    ) {
                        break 'full;
                    }
                    idx += 1;
                }
            }
        }

        reply.ok()
    }
}

fn create_folder_attrs(ino: Ino) -> FileAttr {
    FileAttr {
        ino: ino.0,
        size: 4096,
        blocks: 8,
        atime: UNIX_EPOCH,
        mtime: UNIX_EPOCH,
        ctime: UNIX_EPOCH,
        crtime: UNIX_EPOCH,
        kind: Directory,
        perm: 0o700,
        nlink: 1,
        uid: 1000,
        gid: 1000,
        rdev: 0,
        blksize: 512,
        flags: 0,
    }
}
