use std::cell::RefCell;
use std::cmp::min;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::fs::ReadDir;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Deref;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::rc::Rc;
use std::time::{Duration, UNIX_EPOCH};

use bincode::config;
use bincode::config::Configuration;
use bincode::serde::Compat;
use clap::{Arg, Command, crate_version};
use clap::ArgAction::SetTrue;
use fuser::{FileAttr, Filesystem, FileType, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyDirectoryPlus, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyXattr, Request};
use fuser::FileType::Directory;
use libc::{EBADF, EBADR, EISDIR, ENOENT, ENOSYS, ENOTDIR, ENOTSUP, regex_t};
use log::{debug, info, LevelFilter};
use pretty_env_logger::env_logger::Builder;
use pretty_env_logger::env_logger::fmt::Formatter;
use slotmap::{SecondaryMap, SlotMap};

use crate::index::{AllFiles, File, FileKey, PersistentState, Tag, TagFiles, Tags};

mod index;

const TTL: Duration = Duration::new(0, 0);
// 1 second
const INODE_SPLIT: u64 = 32;
const ROOT_INO: u64 = 1;

// inode
// 64 bit |00000000000000000000000000000000|00000000000000000000000000000000|
// first 32 bits are for tag inode number giving us +- 4 billion tags
// next 32 bits are for file inode number giving us +- 4 billion files per tag
// use the file bits to check if it's a file or a tag (file bits == 0 -> tag)
// this allows us to assign unique inodes to files across folders and still identify them easily.

// this solves issues that file managers may have with recurring inode numbers.

struct HelloFS {
    source_path: String,
    tags: Tags,
    files: AllFiles,
    tag_inode_cache: HashMap<u64, TagNode>,
    // file inode count
    fi_count: u64,
    // tag inode count
    ti_count: u64,
    // file_handle count
    fh_count: u64,
}

impl HelloFS {
    fn save(&self) {
        let config = config::standard();
        save_state(config, &self.files, &self.tags);
    }

    fn get_next_file_handle(&mut self) -> u64 {
        self.fh_count += 1;
        self.fh_count
    }

    fn ino_exists(&mut self, ino: &u64) -> bool {
        return self.tag_inode_cache.contains_key(&ino) || self.files.iter().any(|f| &f.1.inode == ino);
    }
}

// Tag operations
impl HelloFS {
    fn find_tag(&self, tag: &OsStr) -> Option<Tag> {
        self.tags
            .tags
            .iter()
            .find(|(t, _)| t.0 == tag.to_str().unwrap())
            .map(|(t, _)| t.clone())
    }

    fn rename_tag(&mut self, old_name: Tag, new_name: &OsStr) {
        let mut tags = &mut self.tags.tags;
        let index = tags.iter().position(|(t, _)| t == &old_name).unwrap();
        tags[index].0 = Tag::from(new_name.to_str().unwrap());
    }

    fn remove_tag_from_tags(&mut self, tag: Tag) {
        let mut tags = &mut self.tags.tags;
        let index = tags.iter().position(|(t, _)| t == &tag).unwrap();
        tags.remove(index);
    }
}


struct TagNode {
    pub file: File,
    pub metadata: FileAttr,
    // inode of parent tag
    pub parent: Option<u64>,
    // inodes of known children tags
    pub children: Vec<u64>,
}

impl HelloFS {
    fn create_folder_attrs(ino: u64) -> FileAttr {
        FileAttr {
            ino,
            size: 4096,
            blocks: 8,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o700,
            nlink: 1,
            uid: 1000,
            gid: 1000,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }

    fn create_file_attrs(ino: u64, size: u64) -> FileAttr {
        FileAttr {
            ino,
            size,
            blocks: 8,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: 0o700,
            nlink: 1,
            uid: 1000,
            gid: 1000,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }

    pub(crate) fn find_tagged_files(&self, ino: u64) -> Vec<&File> {
        let tags: Vec<Tag> = self.traverse_collect_tags(ino);
        let tag_files_dict = &self.tags.tags;

        // Filters our tag -> files db for the relevant tags of the path
        let relevant_tag_files = tag_files_dict
            .iter()
            .filter(|(tag, _)| tags.contains(tag))
            .map(|(tag, files)| files)
            .collect::<Vec<&Rc<RefCell<TagFiles>>>>();

        if relevant_tag_files.is_empty() {
            return vec![];
        }

        // takes the head off (should be the smallest set) and find the intersection of all other sets
        let (small_set, other_sets) = relevant_tag_files.split_first().unwrap();
        let intersection = small_set
            .borrow()
            .iter()
            .filter(|(file_key, _)| {
                other_sets
                    .iter()
                    .all(|set| set.borrow().contains_key(file_key.clone()))
            })
            .map(|(fk, _)| self.files.get(fk).unwrap())
            .collect::<Vec<&File>>();
        info!("found tagged files: {:?}", intersection);

        return intersection;
    }

    fn lookup_source_file_attrs(&mut self, name: &OsStr) -> Option<FileAttr> {
        let source_path = (&self.source_path).to_string();
        let abs_path = source_path + "/" + name.to_str().unwrap();

        let metadata = fs::metadata(&abs_path);
        if metadata.is_ok() {
            info!("Looked up in source: {}", abs_path);
            let hfs_root_file = self.get_hfs_file_from_name(name.to_str().unwrap()).unwrap();
            let metadata = metadata.unwrap();
            let ctime = metadata.created().unwrap_or(UNIX_EPOCH);
            if metadata.is_dir() {
                return None;
            }
            return Some(FileAttr {
                ino: hfs_root_file.inode,
                size: metadata.size(),
                blocks: metadata.blocks(),
                atime: metadata.accessed().unwrap_or(UNIX_EPOCH), // 1970-01-01 00:00:00
                mtime: metadata.modified().unwrap_or(UNIX_EPOCH),
                ctime,
                crtime: ctime,
                kind: FileType::RegularFile,
                perm: metadata.permissions().mode() as u16,
                nlink: metadata.nlink() as u32,
                uid: metadata.uid(),
                gid: metadata.gid(),
                rdev: metadata.rdev() as u32,
                flags: 0,
                blksize: metadata.blksize() as u32,
            });
        }
        return None;
    }

    // get hello file system file from name
    fn get_hfs_file_from_name(&self, name: &str) -> Option<File> {
        return self
            .files
            .iter()
            .find(|(_, file)| file.name.eq(name))
            .map(|(_, file)| file.clone());
    }

    // recursively traverse the tag_inode_cache upwards to collect the tags
    fn traverse_collect_tags(&self, ino: u64) -> Vec<Tag> {
        if ino == ROOT_INO {
            return vec![];
        }
        let tag_node = &self
            .tag_inode_cache
            .get(&ino)
            .expect("disconnect in ino cache");
        let option = tag_node.parent;
        if option.is_none() {
            return vec![];
        }
        let option = option.unwrap();
        let mut vec = self.traverse_collect_tags(option.clone());
        vec.push(Tag(tag_node.file.name.to_string()));
        return vec;
    }

    fn add_tag_to_file(&mut self, target_dir_ino: u64, fk: FileKey) {
        let tags = self.traverse_collect_tags(target_dir_ino);
        for tag in tags {
            if let Some((_tag, tag_i_files)) =
                self.tags.tags.iter_mut().find(|(tag_i, list)| &tag == tag_i)
            {
                tag_i_files.borrow_mut().insert(fk.clone(), ());
            }
        }
    }
}

impl Filesystem for HelloFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        info!("lookup: parent: {}, name: {:?}", parent, name);
        if parent == ROOT_INO {
            let source_file = &self.lookup_source_file_attrs(name);
            if let Some(source_file) = source_file {
                reply.entry(&TTL, source_file, 0);
                return;
            } else {
                let tag = &self
                    .tags
                    .tags
                    .iter()
                    .find(|(tag, rc)| tag.0 == name.to_str().unwrap());
                if tag.is_none() {
                    /// no tag exists for [name]
                    debug!("no tag exists for [{:?}]", name.to_str().unwrap());
                    reply.error(ENOENT);
                    return;
                }

                let res = &self
                    .tag_inode_cache
                    .iter()
                    .filter(|(_, tag_node)| tag_node.parent == Some(ROOT_INO))
                    .find(|(_, tag_node)| tag_node.file.name == name.to_str().unwrap());

                if let Some((_, tag_node)) = res {
                    reply.entry(&TTL, &tag_node.metadata, 0);
                    return;
                } else {
                    /// Tag exists, but no inode exists for it yet, we play god and create one.
                    let tag_ino = tag_ino_from_count(self.ti_count);
                    let tag_node = TagNode {
                        file: File {
                            fsize: 0,
                            inode: tag_ino,
                            name: name.to_str().unwrap().to_string(),
                        },
                        metadata: HelloFS::create_folder_attrs(tag_ino),
                        parent: Some(ROOT_INO),
                        children: vec![],
                    };
                    self.tag_inode_cache.insert(tag_ino, tag_node);
                    self.ti_count += 1;
                    reply.entry(&TTL, &self.tag_inode_cache[&tag_ino].metadata, 0);
                    return;
                }
            }
        } else {
            let x = self.tag_inode_cache.get(&parent);
            if let Some(tag_node) = x {
                // to get tagged files, intersect all parents their RC<File> lists
                let tagged_files: Vec<&File> = self.find_tagged_files(parent);
                info!("intersection: {:?}", tagged_files);

                let found_file = tagged_files
                    .iter()
                    .find(|file| file.name == name.to_str().unwrap());

                // check if it is a tagged file
                if found_file.is_some() {
                    let found_file = found_file.unwrap();
                    let attr = HelloFS::create_file_attrs(augment_with_tag_ino(found_file.inode, parent), found_file.fsize);
                    reply.entry(&TTL, &attr, 0);
                    return;
                }

                // if not a tagged file, maybe looking for a subtag ?
                // check all children their tagnodes for a matching name, if there: return it
                for child in tag_node.children.iter() {
                    let child_node = self.tag_inode_cache.get(child);
                    if let Some(child_node) = child_node {
                        if child_node.file.name == name.to_str().unwrap() {
                            reply.entry(&TTL, &child_node.metadata, 0);
                            return;
                        }
                    }
                }

                // check parents for matching name, if there: return no entry
                for tag in self.traverse_collect_tags(parent).iter() {
                    if tag.0 == name.to_str().unwrap() {
                        reply.error(ENOENT);
                        return;
                    }
                }

                // check if it's a valid tag at all
                if self.find_tag(name).is_none() {
                    reply.error(ENOENT);
                    return;
                }

                // otherwise we need to create a new tagnode, add it as child to the parent, return it
                let tag_ino = tag_ino_from_count(self.ti_count);
                let tag_node = TagNode {
                    file: File {
                        fsize: 0,
                        inode: tag_ino,
                        name: name.to_str().unwrap().to_string(),
                    },
                    metadata: HelloFS::create_folder_attrs(tag_ino),
                    parent: Some(parent),
                    children: vec![],
                };
                self.tag_inode_cache.insert(tag_ino, tag_node);
                self.ti_count += 1;
                reply.entry(&TTL, &self.tag_inode_cache[&tag_ino].metadata, 0);
                return;
            }
        }

        reply.error(ENOENT);
        return;
    }

    fn getattr(&mut self, _req: &Request, rino: u64, reply: ReplyAttr) {
        let mut ino = rino;
        debug!("Getting attr for: {}", ino);

        if is_file_ino(ino) {
            // drop first 32 bits
            debug!("converted tagged ino into root ino for file: {}", ino);
            ino = ino & !0u32 as u64;
        }

        let x = &self.tag_inode_cache.get(&ino);

        if let Some(file) = x {
            reply.attr(&TTL, &file.metadata);
            return;
        } else if let Some((_, file)) = &self.files.iter().find(|(fk, f)| {
           f.inode == ino
        }) {
            reply.attr(&TTL, &HelloFS::create_file_attrs(rino, file.fsize));
            return;
        }

        if ino == ROOT_INO {
            reply.attr(&TTL, &HelloFS::create_folder_attrs(rino))
        } else {
            reply.error(ENOENT);
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let tag_inode = tag_ino_from_count(self.ti_count);

        let tag_node = TagNode {
            file: File {
                fsize: 0,
                inode: tag_inode,
                name: name.to_str().unwrap().to_string(),
            },
            metadata: HelloFS::create_folder_attrs(tag_inode),
            parent: Some(parent),
            children: vec![],
        };
        self.tag_inode_cache.insert(tag_inode, tag_node);
        self.ti_count += 1;
        reply.entry(&TTL, &self.tag_inode_cache[&tag_inode].metadata, 0);
        self.tags.tags.push((
            Tag(name.to_str().unwrap().to_string()),
            Rc::new(Default::default()),
        ));
        return;
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        if parent == newparent {
            if self.tag_inode_cache.get(&parent).is_some() {
                if let Some(tag) = self.find_tag(name) {
                    self.rename_tag(tag, newname);
                    self.tag_inode_cache.get_mut(&parent).unwrap().file.name = newname.to_str().unwrap().to_string();
                    reply.ok();
                    self.save();
                    return;
                }
            }
        } else if name == newname && parent == ROOT_INO {
            // it's a move from root, so it's a tagging operation on a file
            let file_opt = self.files.iter_mut().find(|(_, file)| name.to_str().unwrap() == file.name);
            if let Some((fk, _file)) = file_opt {
                self.add_tag_to_file(newparent, fk);
                reply.ok();
                self.save();
                return;
            }
        }
        reply.error(ENOTSUP);
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        let ino = if is_file_ino(ino) {
            ino & !0u32 as u64
        } else {
            ino
        };


        if self.ino_exists(&ino) {
            debug!("Opened file: {}", ino);
            reply.opened(self.get_next_file_handle(), 0);
        } else {
            debug!("File does not exist: {}", ino);
            reply.error(ENOENT);
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        info!("Trying to read: {} from {}", ino, offset);
        assert!(offset >= 0);
        if !is_file_ino(ino) {
            reply.error(EISDIR);
            return;
        }

        let mut ino = ino;
        // drop tag bits
        ino = ino & !0u32 as u64;

        let mut optional_path: Option<String> = None;
        for (_key, file) in self.files.iter() {
            if file.inode == ino {
                optional_path = Some((&self.source_path).to_string()+ "/" + file.name.as_str()) ;
            }
        }

        if let Some(path) = optional_path {
            if let Ok(file) = fs::File::open(&path) {
                debug!("Opened file for reading: {}", path);
                let file_size = file.metadata().unwrap().len();
                let read_size = min(size, file_size.saturating_sub(offset as u64) as u32);

                let mut buffer = vec![0; read_size as usize];
                file.read_exact_at(&mut buffer, offset as u64).unwrap();
                reply.data(&buffer);
            } else {
                reply.error(ENOENT);
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn release(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        let ino = if is_file_ino(ino) {
            ino & !0u32 as u64
        } else {
            ino
        };

        if self.ino_exists(&ino) {
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }

    fn opendir(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        if self.ino_exists(&ino) {
            reply.opened(self.get_next_file_handle(), 0);
        } else {
            reply.error(ENOENT);
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir called with ino: {}, offset: {}", ino, offset);
        let tag_node = self.tag_inode_cache.get(&ino);
        if tag_node.is_none() {
            reply.error(ENOENT);
            debug!("readdir exited: no such directory");
            return;
        } else if tag_node.unwrap().metadata.kind != Directory {
            reply.error(ENOTDIR);
            debug!("readdir exited: not a directory");
            return;
        }

        let tag_node = tag_node.unwrap();
        let parent_dir = tag_node.parent.unwrap_or(ROOT_INO);

        let mut entries = vec![(ino, Directory, "."), (parent_dir, Directory, "..")];

        let path_tags = self.traverse_collect_tags(ino);
        for (tag, _) in self.tags.tags.iter() {
            // skip path tags
            if path_tags.contains(tag) {
                continue;
            }
            let (_ino, tag_node) = self
                .tag_inode_cache
                .iter()
                .find(|(inode, tag_node)| tag_node.file.name == tag.0)
                .unwrap();

            entries.push((tag_node.file.inode, Directory, tag_node.file.name.as_str()));
        }

        let relevant_files = if ino == ROOT_INO {
            self.files.iter().map(|t| t.1).collect::<Vec<&File>>()
        } else {
            self.find_tagged_files(ino)
        };

        for file in relevant_files {
            entries.push((augment_with_tag_ino(file.inode, ino), FileType::RegularFile, file.name.as_str()));
        }

        entries.drain(0..offset as usize);

        debug!("readdir returning: {:?}", entries.iter().skip(offset as usize).collect::<Vec<&(u64, FileType, &str)>>());

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            // i + 1 means the index of the next entry
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
        }

        reply.ok();
    }

    fn releasedir(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, _flags: i32, reply: ReplyEmpty) {
        if self.ino_exists(&ino) {
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        debug!("statfs called");
        reply.statfs(0, 0, 0, 7, 0, 512, 255, 0);
    }

    fn getxattr(&mut self, _req: &Request<'_>, ino: u64, name: &OsStr, size: u32, reply: ReplyXattr) {
        debug!("Getting xattr for: {}", ino);
        reply.size(0);
    }

    fn access(&mut self, _req: &Request<'_>, ino: u64, _mask: i32, reply: ReplyEmpty) {
        let ino = if is_file_ino(ino) {
            ino & !0u32 as u64
        } else {
            ino
        };

        if self.ino_exists(&ino) {
            debug!("Access granted for: {}", ino);
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
}

const fn augment_with_tag_ino(file_ino: u64, tag_ino: u64) -> u64 {
    if tag_ino == ROOT_INO {
        return file_ino;
    }
    return tag_ino | file_ino;
}

const fn tag_ino_from_count(count: u64) -> u64 {
    return count << INODE_SPLIT;
}

const fn is_file_ino(ino: u64) -> bool {
    return if ino == ROOT_INO {
        false
    } else {
        (ino & !0u32 as u64) != 0
    };
}

fn main() {
    let config = config::standard();
    let example_tags: Vec<&str> = vec!["all", "five"];

    setup_logger();

    let matches = Command::new("hello")
        .version(crate_version!())
        .author("me")
        .arg(
            Arg::new("MOUNT_POINT")
                .required(true)
                .index(1)
                .help("Act as a client, and mount FUSE at given path"),
        )
        .arg(
            Arg::new("SOURCE")
                .index(2)
                .help("Source files from here, read only"),
        )
        .arg(
            Arg::new("auto-unmount")
                .long("auto-unmount")
                .help("Automatically unmount on process exit")
                .action(SetTrue),
        )
        .arg(
            Arg::new("allow-root")
                .long("allow-root")
                .help("Allow root user to access filesystem")
                .action(SetTrue),
        )
        .get_matches();

    let mountpoint: &String = matches.get_one("MOUNT_POINT").unwrap();
    let source_path: &String = matches.get_one("SOURCE").unwrap();
    let paths = fs::read_dir(source_path).unwrap();

    let mut options = vec![
        MountOption::FSName("miauw".to_string()),
    ];

    if matches.get_flag("auto-unmount") {
        options.push(MountOption::AutoUnmount);
        options.push(MountOption::AllowRoot);
    }

    let mut files = AllFiles::default();

    let tag_files = if let Ok(file) = fs::read("savefile") {
        let (Compat(decoded), _len): (Compat<PersistentState>, usize) =
            bincode::decode_from_slice(&file[..], config).unwrap();
        files = decoded.files;
        decoded.tags
    } else {
        let mut tags = vec![];
        let mut map = SecondaryMap::new();

        for (file_key, _) in &files {
            map.insert(file_key, ());
        }

        for tag in &example_tags {
            tags.push((tag.to_string().into(), Rc::new(RefCell::new(map.clone()))));
        }

        Tags { tags }
    };

    // file inode counter, 0 is reserved for tag/file detection, 1 for root dir.
    let mut fi_count = 2;

    load_files(source_path, paths, &mut files, &mut fi_count);

    let mut inode_cache = HashMap::new();
    let mut all_tag_inodes = vec![];

    // tag inode counter, 0 would cause confusion with files in the root dir,
    // as they don't have tag padding (first 32 bits are 0).
    let mut ti_count = 1;

    for tag in example_tags {
        let tag_inode = tag_ino_from_count(ti_count);
        inode_cache.insert(
            tag_inode,
            TagNode {
                file: File {
                    fsize: 0,
                    name: tag.to_string(),
                    inode: tag_inode,
                },
                metadata: HelloFS::create_folder_attrs(tag_inode),
                parent: Some(ROOT_INO),
                children: vec![],
            },
        );
        all_tag_inodes.push(tag_inode);
        ti_count += 1
    }
    inode_cache.insert(
        ROOT_INO,
        TagNode {
            file: File {
                fsize: 0,
                name: "mount".to_string(),
                inode: ROOT_INO,
            },
            metadata: HelloFS::create_folder_attrs(ROOT_INO),
            parent: None,
            children: all_tag_inodes,
        },
    );

    info!("Loaded: {} files from `{}`", fi_count, source_path);
    info!("Inode cache: {:?}", &inode_cache.keys());

    let encoded = save_state(config, &mut files, &tag_files);
    fs::write("savefile", encoded).expect("couldn't save");
    info!("saved file");

    let filesystem = HelloFS {
        source_path: source_path.to_string(),
        tags: tag_files,
        files,
        tag_inode_cache: inode_cache,
        fi_count,
        ti_count,
        fh_count: 0,
    };

    fuser::mount2(filesystem, mountpoint, &options).unwrap();
}

fn save_state(config: Configuration, files: &SlotMap<FileKey, File>, tag_files: &Tags) -> Vec<u8> {
    let persistent_state = PersistentState {
        files: files.clone(),
        tags: tag_files.clone(),
    };
    let encoded: Vec<u8> = bincode::encode_to_vec(Compat(&persistent_state), config).unwrap();
    encoded
}

fn setup_logger() {
    // Create a new `env_logger::Builder`
    let mut builder = Builder::new();

    // Set the minimum log level to `Debug`
    builder.filter_level(LevelFilter::Debug);

    // Configure the log format
    builder.format_timestamp_secs();

    // Initialize the logger
    builder.init();
}

fn load_files(
    source_path: &String,
    paths: ReadDir,
    files: &mut SlotMap<FileKey, File>,
    i: &mut u64,
) {
    for (_key, _loaded_file) in files.iter() {}

    for path in paths {
        let path = path.unwrap().path();
        let file_name = path
            .to_str()
            .unwrap()
            .strip_prefix(source_path)
            .unwrap()
            .strip_prefix("/")
            .unwrap()
            .to_string();
        info!("{}", file_name);

        let source_path = (source_path).to_string();
        let abs_path = source_path + "/" + file_name.as_str();
        let metadata = fs::metadata(abs_path);
        let file = File {
            fsize: metadata.unwrap().len(),
            name: file_name,
            inode: *i,
        };

        let loaded = files.iter().any(|(_key, loaded_file)| {
            loaded_file.fsize == file.fsize && loaded_file.name == file.name
        });

        if loaded {
            continue;
        }
        files.insert(file);

        *i += 1;
    }
}

#[cfg(test)]
mod test {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fuser::FileAttr;
    use slotmap::SlotMap;

    use crate::{HelloFS, ROOT_INO, TagNode};
    use crate::index::{AllFiles, File, Tag, TagFiles, Tags};

    #[test]
    fn test_intersections() {
        /// files
        /// - /  (1)
        ///   - file1 (2)
        ///   - file2 (3)
        let file1 = File {
            fsize: 5,
            name: "file1".to_string(),
            inode: 2,
        };
        let file2 = File {
            fsize: 5,
            name: "file2".to_string(),
            inode: 3,
        };

        let mut files: AllFiles = AllFiles::default();
        let fk_1 = files.insert(file1.clone());
        let fk_2 = files.insert(file2.clone());

        let mut tag1_files = TagFiles::new();
        tag1_files.insert(fk_1, ());
        tag1_files.insert(fk_2, ());

        let mut tag2_files = TagFiles::new();

        /// Inode cache:
        /// - /  (1)
        ///   - tag1 (4)
        ///   - tag2 (5)
        ///     - tag1 | 6
        let mut inode_cache = HashMap::new();
        inode_cache.insert(
            1,
            TagNode {
                file: File {
                    fsize: 4096,
                    name: "mount".to_string(),
                    inode: 1,
                },
                metadata: HelloFS::create_folder_attrs(ROOT_INO),
                parent: None,
                children: vec![5, 6],
            },
        );

        inode_cache.insert(
            4,
            TagNode {
                file: File {
                    fsize: 5,
                    name: "tag1".to_string(),
                    inode: 4,
                },
                metadata: HelloFS::create_folder_attrs(4),
                parent: Some(ROOT_INO),
                children: vec![],
            },
        );

        inode_cache.insert(
            5,
            TagNode {
                file: File {
                    fsize: 5,
                    name: "tag2".to_string(),
                    inode: 5,
                },
                metadata: HelloFS::create_folder_attrs(5),
                parent: Some(ROOT_INO),
                children: vec![6],
            },
        );

        inode_cache.insert(
            6,
            TagNode {
                file: File {
                    fsize: 5,
                    name: "tag1".to_string(),
                    inode: 6,
                },
                metadata: HelloFS::create_folder_attrs(6),
                parent: Some(5),
                children: vec![],
            },
        );

        let hi = HelloFS {
            source_path: "/tmp/testing_tagfs".to_string(),
            tags: Tags {
                tags: vec![
                    ("tag1".into(), Rc::new(RefCell::from(tag1_files))),
                    ("tag2".into(), Rc::new(RefCell::from(tag2_files))),
                ],
            },
            files,
            tag_inode_cache: inode_cache,
            ti_count: 3,
            fi_count: 1,
            fh_count: 0,
        };

        assert_eq!(
            hi.traverse_collect_tags(6),
            vec!["tag2".into(), "tag1".into()]
        );
        assert_eq!(hi.find_tagged_files(4), vec![&file1, &file2]);
    }
}
