use std::cmp::min;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::iter::Map;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::rc::Rc;
use std::time::{Duration, UNIX_EPOCH};

use clap::{Arg, Command, crate_version};
use clap::ArgAction::SetTrue;
use fuser::{FileAttr, Filesystem, FileType, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, Request};
use libc::ENOENT;
use slotmap::{Key, SecondaryMap};

use crate::index::{AllFiles, File, FileKey, Tags};

mod index;

const TTL: Duration = Duration::from_secs(1); // 1 second

struct HelloFS {
    source_path: String,
    tags: Tags,
    files: AllFiles,
    tag_inode_cache: HashMap<u64, AllFileInformation>,
    inode_count: u64
}

struct AllFileInformation {
    file: File,
    metadata: FileAttr,
}

impl HelloFS {
    fn get_hfs_file_from_name(&self, name: &str) -> Option<File> {
        return self.files.iter()
            .find(|(_, file)| file.name.eq(name))
            .map(|(_, file)| file.clone());
    }
}

impl Filesystem for HelloFS {

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        // // read files from HelloFS.files and present them as files in the filesystem
        // // read tags from HelloFS.tags and present them as directories in the filesystem
        //
        // for (tag, files) in self.tags.tags.iter() {
        //     if parent == 1 && name.to_str() == Some(tag.as_str()) {
        //         reply.entry(&TTL, &HELLO_DIR_ATTR, 0);
        //     }
        // }

        let source_path = (&self.source_path).to_string();
        let abs_path = source_path + "/" + name.to_str().unwrap();

        let metadata = fs::metadata(&abs_path);
        if metadata.is_ok() {
            println!("Looked up: {}", abs_path);
            let hfs_root_file = self.get_hfs_file_from_name(name.to_str().unwrap())
                .unwrap();
            let metadata = metadata.unwrap();
            let ctime = metadata.created().unwrap_or(UNIX_EPOCH);
            if metadata.is_dir() {
                return;
            }

            reply.entry(&TTL, &FileAttr {
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
            }, 0);
            return;
        } else {
            let tag = &self.tags.tags.iter().find(|(tag, rc)| {
                tag.0 == name.to_str().unwrap()
            });
            if tag.is_some() {
                reply.entry(&TTL, &FileAttr {
                    ino: 5,
                    size: 5,
                    blocks: 5,
                    atime: UNIX_EPOCH,
                    mtime: UNIX_EPOCH,
                    ctime: UNIX_EPOCH,
                    crtime: UNIX_EPOCH,
                    kind: FileType::Directory,
                    perm: 0o755,
                    nlink: 1,
                    uid: 1000,
                    gid: 1000,
                    rdev: 0,
                    blksize: 512,
                    flags: 0,
                }, 0);
                return;
            }

            println!("Error: {:?}", metadata.err());
            reply.error(ENOENT);
            return;
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        let x = &self.tag_inode_cache.get(&ino);
        println!("Getting attr for: {}", ino);
        if let Some(file) = x {
            reply.attr(&TTL, &file.metadata);
            return;
        }

        if ino == 1 {
            reply.attr(&TTL, &FileAttr {
                ino,
                size: 5,
                blocks: 5,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 1,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                blksize: 512,
                flags: 0,
            })
        } else {
            reply.error(ENOENT);
        }
    }

    fn access(&mut self, _req: &Request<'_>, ino: u64, mask: i32, reply: ReplyEmpty) {
        println!("Accessing: {}", ino);
        reply.ok();
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
        println!("Trying to read: {} from {}", ino, offset);
        for (_key, file) in self.files.iter() {
            if file.inode == ino {
                let source_path = (&self.source_path).to_string();
                let abs_path = source_path + "/" + file.name.as_str();

                let size = min(file.fsize - offset as u64, size as u64);

                let mut buf = Vec::new();
                buf.resize(size as usize, 0);
                println!("allocated buffer with: {} len", size);

                let mut sys_file = fs::File::open(abs_path).expect("Error opening file");
                sys_file.seek(SeekFrom::Start(offset as u64))
                    .expect("offset might be outside of file");

                let mut_slice = buf.as_mut_slice();
                sys_file.read_exact(mut_slice).expect("Error reading file");

                println!("Reading: {} {}", file.inode, file.name);

                println!("Skipped {:?}, read bytes: {:?}", offset, mut_slice.len());
                reply.data(mut_slice);
                return;
            }
        }

        reply.error(ENOENT);
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        println!("readdir called with ino: {}", ino);

        let mut entries = vec![
            (1, FileType::Directory, "."),
            (1, FileType::Directory, ".."),
        ];

        for entry in self.tag_inode_cache.iter() {
            let tag = &entry.1;
            entries.push((tag.file.inode, FileType::Directory, tag.file.name.as_str()));
        }
        //
        // for (tag, _files) in self.tags.tags.iter() {
        //     let name = &tag.0;
        //
        //     entries.push((9000, FileType::Directory, name));
        // }

        for (_file_key, file) in self.files.iter() {
            entries.push((file.inode, FileType::RegularFile, file.name.as_str()));
        }

        println!("{:?}", entries);

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            // i + 1 means the index of the next entry
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
    }
}


fn main() {
    let matches = Command::new("hello")
        .version(crate_version!())
        .author("Christopher Berner")
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
                .action(SetTrue)
        )
        .arg(
            Arg::new("allow-root")
                .long("allow-root")
                .help("Allow root user to access filesystem")
                .action(SetTrue),
        )
        .get_matches();

    env_logger::init();

    let mountpoint: &String = matches.get_one("MOUNT_POINT").unwrap();
    let source_path: &String = matches.get_one("SOURCE").unwrap();
    let paths = fs::read_dir(source_path).unwrap();

    let mut options = vec![MountOption::RO, MountOption::FSName("hello".to_string())];

    if matches.get_flag("auto-unmount") {
        options.push(MountOption::AutoUnmount);
    }
    if matches.get_flag("allow-root") {
        options.push(MountOption::AllowRoot);
    }

    let mut files = AllFiles::default();
    let mut map = SecondaryMap::new();

    let mut i = 2; // inode 1 is the root dir
    for path in paths {
        let path = path.unwrap().path();
        let file_name = path.to_str().unwrap().strip_prefix(source_path).unwrap().strip_prefix("/").unwrap().to_string();
        println!("{}", file_name);

        let source_path = (source_path).to_string();
        let abs_path = source_path + "/" + file_name.as_str();
        let metadata = fs::metadata(abs_path);
        let file = File {
            fsize: metadata.unwrap().len(),
            name: file_name,
            inode: i,
        };
        let file_key = files.insert(file);
        map.insert(file_key, ());
        i += 1;
    }

    let tags = vec!["all"];
    let mut inode_cache = HashMap::new();
    for tag in tags {
        inode_cache.insert(i, AllFileInformation {
            file: File {
                fsize: 0,
                name: tag.to_string(),
                inode: i,
            },
            metadata: FileAttr {
                ino: i,
                size: 0,
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 1,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                blksize: 512,
                flags: 0,
            },
        });
        i += 1;
    }

    println!("Loaded: {} files from `{}`", i, source_path);
    println!("Inode cache: {:?}", &inode_cache.keys());

    let filesystem = HelloFS {
        source_path: source_path.to_string(),
        tags: Tags {
            tags: vec![("all".into(), Rc::new(map))],
        },
        files,
        tag_inode_cache: inode_cache,
        inode_count: i
    };
    fuser::mount2(filesystem, mountpoint, &options).unwrap();
}