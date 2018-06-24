// FIXME: Refactor error
use atomic_immut::AtomicImmut;
use config::{Config, Operation, Speed};
use fuse::{self, *};
use libc;
use localfile::{Inode, LocalFile};
use metrics::Metrics;
use slog::Logger;
use state::{State, StateManager};
use std;
use std::collections::HashMap;
use std::error::Error;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem;
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::result::Result;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;
use time::{PreciseTime, Timespec};

type FileHandler = u64;

const TTL: Timespec = Timespec { sec: 1, nsec: 0 };
const ROOT_DIR_INO: u64 = 1;

pub struct Mizumochi {
    logger: Logger,

    state_manager: StateManager,
    config: Arc<AtomicImmut<Config>>,

    // FIXME: use simple allocator.
    ino_count: Inode,
    fh_count: FileHandler,

    fh_map: HashMap<FileHandler, File>,
    file_map: HashMap<Inode, LocalFile>,

    original_dir: PathBuf,
    mountpoint: PathBuf,

    metrics: Metrics,
}

impl Mizumochi {
    pub fn new(
        logger: Logger,
        original_dir: PathBuf,
        mountpoint: PathBuf,
        config: Arc<AtomicImmut<Config>>,
    ) -> Mizumochi {
        let cond = (&*config.load()).condition.clone();
        let state_manager = StateManager::new(cond);

        Mizumochi {
            logger,

            state_manager,
            config,

            fh_count: 1,
            // inode number begins from the next of `ROOT_DIR_INO`.
            ino_count: ROOT_DIR_INO + 1,
            fh_map: HashMap::new(),
            file_map: HashMap::new(),

            mountpoint,
            original_dir,

            metrics: Metrics::new(),
        }
    }

    pub fn mount(self) -> Result<(), io::Error> {
        let mountpoint = self.mountpoint.clone();
        fuse::mount(self, &mountpoint, &[])
    }

    fn init(&mut self) -> Result<(), io::Error> {
        if !self.original_dir.is_dir() {
            error!(
                self.logger,
                "Original filepath is not directory: {:?}", self.original_dir
            );
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Not directory"));
        }

        // Initialize the state.
        self.state_manager.init();
        info!(self.logger, "State: {:?}", self.state_manager.state());

        let path = self.original_dir.clone();
        self.fetch_files_if_not_found(ROOT_DIR_INO, &path)?;

        Ok(())
    }

    fn fetch_files_if_not_found(
        &mut self,
        root_ino: Inode,
        root_dir: &PathBuf,
    ) -> Result<(), io::Error> {
        if !root_dir.is_dir() {
            error!(
                self.logger,
                "fetch_files_if_not_found error: path: {:?} is directory, ino: {}",
                root_dir,
                root_ino
            );
            return Err(io::Error::new(io::ErrorKind::Other, "Not directory"));
        }

        if let Some(LocalFile::Directory(_, Some(_))) = self.file_map.get(&root_ino) {
            // Already fetched.
            return Ok(());
        }

        info!(
            self.logger,
            "fetch_files_if_not_found: ino: {}, path: {:?}", root_ino, root_dir
        );

        let mut files = Vec::new();

        // Fetch the all files in the directory.
        for entry in fs::read_dir(root_dir)? {
            let entry = entry?;
            let path = entry.path();

            let filename = path
                .file_name()
                .ok_or(io::Error::new(io::ErrorKind::Other, "Cannot get filename"))?;

            let file = if path.is_dir() {
                // The files in the directory is loaded later (see lookup).
                LocalFile::Directory(path.clone().into(), None)
            } else {
                LocalFile::RegularFile(path.clone().into())
            };

            let ino = self.ino_count;
            self.ino_count += 1;
            self.file_map.insert(ino, file);

            files.push((ino, filename.into()));
        }

        self.file_map
            .insert(root_ino, LocalFile::Directory(root_dir.into(), Some(files)));

        Ok(())
    }

    fn change_state_if_necessary(&mut self, op: Operation) -> &State {
        let prev_state = self.state_manager.state().clone();

        {
            let cond = &self.config.load().condition;
            let state = if let Ok(state) = self.state_manager.on_operated_after(op, cond) {
                state.clone()
            } else {
                crit!(
                    self.logger,
                    "change_state_if_necessary crit: let the state stable"
                );
                State::Stable
            };

            match (prev_state, state) {
                (State::Stable, State::Unstable) => {
                    self.metrics.speed_limit_enabled.increment();
                    info!(self.logger, "--- Enable unstable mode ---")
                }
                (State::Unstable, State::Stable) => {
                    self.metrics.speed_limit_enabled.increment();
                    info!(self.logger, "--- Enable stable mode ---")
                }
                _ => {}
            }
        }

        self.state_manager.state()
    }

    fn lookup(&mut self, parent: u64, name: &OsStr) -> Result<FileAttr, io::Error> {
        let (inode, path) = match self
            .file_map
            .get(&parent)
            .ok_or(io::Error::new(io::ErrorKind::NotFound, ""))?
        {
            LocalFile::Directory(_, None) => {
                Err(io::Error::new(io::ErrorKind::Other, "not fetched"))
            }
            LocalFile::Directory(path, Some(files)) => {
                // Find the file by the given name.
                let f = files
                    .iter()
                    .find(|(_, path)| Some(name) == path.file_name());

                let (inode, _) = f.ok_or(io::Error::new(io::ErrorKind::NotFound, ""))?;

                let mut path = path.clone();
                path.push(name);

                Ok((*inode, path))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "it is not directory",
            )),
        }?;

        if path.is_dir() {
            self.fetch_files_if_not_found(inode, &path)?;
        }

        fetch_fileattr(inode, &path)
    }

    fn read(&mut self, fh: u64, buffer: &mut [u8], offset: i64, size: u32) -> Result<usize, c_int> {
        let logger = &self.logger;
        let f = self.fh_map.get_mut(&fh).ok_or(libc::ENOENT)?;

        let file_size = f.metadata().map_err(|_| libc::EIO)?.len();

        let offset = offset as u64;
        if offset < file_size {
            if let Err(error) = f.seek(SeekFrom::Start(offset)) {
                error!(logger, "seek error {}", error);
                return Err(libc::EIO);
            }

            // Truncate the size to avoid overreading.
            let size = if file_size < (offset + u64::from(size)) {
                (file_size - offset) as usize
            } else {
                size as usize
            };

            f.read(&mut buffer[0..size]).map_err(|error| {
                error!(logger, "read error {}", error);
                libc::EIO
            })
        } else {
            Ok(0)
        }
    }

    fn write(&mut self, fh: u64, buffer: &[u8], offset: i64) -> Result<usize, c_int> {
        let logger = &self.logger;
        let f = self.fh_map.get_mut(&fh).ok_or(libc::ENOENT)?;
        if let Err(error) = f.seek(SeekFrom::Start(offset as u64)) {
            error!(self.logger, "seek error {}", error);
            return Err(libc::EIO);
        }

        let written_size = f.write(buffer).or_else(|error| {
            error!(logger, "write error {}", error);
            Err(libc::EIO)
        })?;

        // Reflect the written result to the actual file.
        let _ = f.sync_all().or_else(|error| {
            error!(logger, "write error {}", error);
            Err(libc::EIO)
        })?;
        let _ = f.sync_data().or_else(|error| {
            error!(logger, "write error {}", error);
            Err(libc::EIO)
        })?;

        Ok(written_size)
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _: u64,
        offset: i64,
        reply: &mut ReplyDirectory,
    ) -> Result<(), io::Error> {
        if offset != 0 {
            // From the FUSE document https://libfuse.github.io/doxygen/structfuse__operations.html#ae269583c4bfaf4d9a82e1d51a902cd5c
            // Filesystem can ignore the offset.
            // > 1) The readdir implementation ignores the offset parameter, and passes zero to the filler function's offset.
            // > The filler function will not return '1' (unless an error happens), so the whole directory is read in a single readdir operation.
            return Ok(());
        }

        let files = match self.file_map.get(&ino) {
            Some(LocalFile::Directory(_, Some(files))) => Ok(files),
            _ => Err(io::Error::new(io::ErrorKind::NotFound, "")),
        }?;

        // Add itself and the parent.
        reply.add(1, 0, FileType::Directory, ".");
        reply.add(1, 1, FileType::Directory, "..");

        // Add the files in the directory.
        let mut offset = 2i64;
        for (fino, _) in files {
            let (ftype, path) = match self.file_map.get(&fino) {
                Some(LocalFile::RegularFile(path)) => (FileType::RegularFile, path),
                Some(LocalFile::Directory(path, _)) => (FileType::Directory, path),
                None => {
                    crit!(self.logger, "file_map is inconsistent: {:?}", self.file_map);
                    crit!(self.logger, "directory ino: {}, file ino: {}", ino, fino);
                    return Err(io::Error::new(io::ErrorKind::Other, "meybe bug"));
                }
            };

            let filename = path
                .file_name()
                .ok_or(io::Error::new(io::ErrorKind::Other, "Cannot get filename"))?;
            reply.add(*fino, offset, ftype, filename);

            offset += 1;
        }

        Ok(())
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _flags: u32,
    ) -> Result<(FileAttr, FileHandler), io::Error> {
        let name = name
            .to_str()
            .ok_or(io::Error::new(io::ErrorKind::Other, ""))?;

        let (attr, fh, ino, f) = {
            let (mut path, files) = match self.file_map.get_mut(&parent) {
                Some(LocalFile::Directory(path, Some(files))) => Ok((path.clone(), files)),
                _ => Err(io::Error::new(io::ErrorKind::Other, "")),
            }?;

            path.push(name);

            let file = File::create(&path)?;

            let ino = self.ino_count;
            self.ino_count += 1;

            let attr = fetch_fileattr(ino, &path)?;

            let fh = self.fh_count;
            self.fh_count += 1;

            self.fh_map.insert(fh, file);
            files.push((ino, name.into()));

            (attr, fh, ino, LocalFile::RegularFile(path.clone()))
        };

        self.file_map.insert(ino, f);

        Ok((attr, fh))
    }
}

impl Filesystem for Mizumochi {
    fn init(&mut self, _req: &Request) -> Result<(), c_int> {
        info!(self.logger, "init");

        Mizumochi::init(self).map_err(|error| {
            error!(self.logger, "init error: {}", error.description());
            libc::EIO
        })
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!(self.logger, "lookup: parent: {}, name: {:?}", parent, name);
        self.metrics.io_operations_lookup.increment();

        match Mizumochi::lookup(self, parent, name) {
            Ok(ref attr) => reply.entry(&TTL, attr, 0),
            Err(error) => match error.kind() {
                io::ErrorKind::NotFound => reply.error(libc::ENOENT),
                _ => {
                    error!(self.logger, "lookup error: {}", error.description());
                    reply.error(libc::EIO)
                }
            },
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        debug!(self.logger, "getattr: ino: {:?}", ino);
        self.metrics.io_operations_getattr.increment();

        match self.file_map.get(&ino) {
            Some(LocalFile::RegularFile(path)) => match fetch_fileattr(ino, path) {
                Ok(attr) => reply.attr(&TTL, &attr),
                Err(error) => {
                    error!(
                        self.logger,
                        "getattr error: ino = {}, path = {:?}, error = {}", ino, path, error
                    );
                    reply.error(libc::EIO)
                }
            },
            Some(LocalFile::Directory(path, _)) => match fetch_fileattr(ino, path) {
                Ok(attr) => reply.attr(&TTL, &attr),
                Err(error) => {
                    error!(
                        self.logger,
                        "getattr error: ino = {}, path = {:?}, error = {}", ino, path, error
                    );
                    reply.error(libc::EIO)
                }
            },
            _ => {
                reply.error(libc::ENOENT);
            }
        }
    }

    fn readdir(
        &mut self,
        req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!(
            self.logger,
            "readdir: ino: {}, fh: {}, offset: {}", ino, fh, offset
        );
        self.metrics.io_operations_readdir.increment();

        use self::io::ErrorKind;
        if let Err(error) = self.readdir(req, ino, fh, offset, &mut reply) {
            let e = match error.kind() {
                ErrorKind::NotFound => libc::ENOENT,
                _ => {
                    error!(self.logger, "readdir error: {}", error.description());
                    libc::EIO
                }
            };

            reply.error(e);
        } else {
            reply.ok();
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        reply: ReplyData,
    ) {
        debug!(
            self.logger,
            "read: ino: {}, fh: {}, offset: {}, size: {}", ino, fh, offset, size
        );
        self.metrics.io_operations_read.increment();

        let start = PreciseTime::now();

        let mut buffer = vec![0; size as usize];

        match Mizumochi::read(self, fh, &mut buffer, offset, size) {
            Ok(read_size) => {
                reply.data(&buffer[0..read_size]);

                if State::Unstable == *self.change_state_if_necessary(Operation::Read) {
                    if let Speed::Bps(bps) = self.config.load().speed {
                        // Mesure elapsed time and wait if necessary.
                        sleep(compute_sleep_duration_to_adjust_speed(
                            bps,
                            read_size,
                            start.to(PreciseTime::now()).num_milliseconds() as u64,
                        ));
                    }
                }
            }
            Err(error) => {
                error!(self.logger, "read error: {}", error);
                reply.error(libc::EIO);
            }
        }
    }

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<Timespec>,
        _mtime: Option<Timespec>,
        fh: Option<u64>,
        _crtime: Option<Timespec>,
        _chgtime: Option<Timespec>,
        _bkuptime: Option<Timespec>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!(self.logger, "setattr: ino: {}, fh: {:?}", ino, fh);
        self.metrics.io_operations_setattr.increment();

        match self.file_map.get(&ino) {
            Some(LocalFile::RegularFile(path)) => match fetch_fileattr(ino, path) {
                Ok(attr) => reply.attr(&TTL, &attr),
                Err(error) => {
                    error!(
                        self.logger,
                        "getattr error: ino = {}, path = {:?}, error = {}", ino, path, error
                    );
                    reply.error(libc::EIO)
                }
            },
            Some(LocalFile::Directory(path, _)) => match fetch_fileattr(ino, path) {
                Ok(attr) => reply.attr(&TTL, &attr),
                Err(error) => {
                    error!(
                        self.logger,
                        "getattr error: ino = {}, path = {:?}, error = {}", ino, path, error
                    );
                    reply.error(libc::EIO)
                }
            },
            _ => {
                reply.error(libc::ENOENT);
            }
        }
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _flags: u32,
        reply: ReplyWrite,
    ) {
        debug!(
            self.logger,
            "write: ino: {}, fh: {}, offset: {}, size: {}",
            ino,
            fh,
            offset,
            data.len()
        );
        self.metrics.io_operations_write.increment();

        let start = PreciseTime::now();

        match Mizumochi::write(self, fh, data, offset) {
            Ok(written_size) => {
                reply.written(written_size as u32);

                if State::Unstable == *self.change_state_if_necessary(Operation::Write) {
                    if let Speed::Bps(bps) = self.config.load().speed {
                        sleep(compute_sleep_duration_to_adjust_speed(
                            bps,
                            written_size,
                            start.to(PreciseTime::now()).num_milliseconds() as u64,
                        ));
                    }
                }
            }
            Err(ecode) => {
                error!(self.logger, "  read error: {:?}", ecode);
                reply.error(ecode);
            }
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: u32, reply: ReplyOpen) {
        // TODO: handle the flags.
        info!(self.logger, "open ino: {}, flags: {}", ino, flags);
        self.metrics.io_operations_open.increment();

        match self.file_map.get(&ino) {
            Some(LocalFile::RegularFile(filepath)) => {
                let mut options = fs::OpenOptions::new();
                options.read(true).write(true).create(false);

                info!(self.logger, "filepath: {:?}", filepath);
                match options.open(filepath) {
                    Ok(f) => {
                        let fh = self.fh_count;
                        self.fh_count += 1;
                        self.fh_map.insert(fh, f);

                        reply.opened(fh, 0);
                    }
                    Err(error) => {
                        error!(self.logger, "open error: {}", error);
                        reply.error(libc::EIO)
                    }
                }
            }
            Some(LocalFile::Directory(filepath, _)) => {
                error!(self.logger, "directory: {:?}", filepath);
                reply.error(libc::ENOENT)
            }
            None => {
                error!(self.logger, "readdir error: inode {} is not found", ino);
                reply.error(libc::ENOENT)
            }
        }
    }

    fn flush(&mut self, _req: &Request, ino: u64, fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        debug!(self.logger, "flush: ino: {}, fh: {}", ino, fh);
        self.metrics.io_operations_flush.increment();

        if let Some(f) = self.fh_map.get_mut(&fh) {
            if let Err(error) = f.seek(SeekFrom::Start(0)) {
                info!(self.logger, "flush seek error: {}", error);
                reply.error(libc::EIO);
            } else {
                reply.ok();
            }
        } else {
            error!(self.logger, "flush error: no entry");
            reply.error(libc::ENOENT);
        }
    }

    fn release(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        info!(self.logger, "release: ino: {}, fh: {}", ino, fh);
        self.metrics.io_operations_release.increment();

        if let Some(f) = self.fh_map.remove(&fh) {
            if let Err(error) = f.sync_data() {
                error!(self.logger, "sync_data error: {}", error);
                reply.error(libc::EIO);
            } else {
                reply.ok();
            }
        } else {
            error!(self.logger, "release error: no entry");
            reply.error(libc::ENOENT);
        }
    }

    fn fsync(&mut self, _req: &Request, ino: u64, fh: u64, datasync: bool, reply: ReplyEmpty) {
        debug!(
            self.logger,
            "fsync ino: {}, fh: {}, datasync: {}", ino, fh, datasync
        );
        self.metrics.io_operations_fsync.increment();

        if let Some(f) = self.fh_map.get(&fh) {
            if let Err(error) = f.sync_data() {
                error!(self.logger, "sync_data error: {}", error);
                reply.error(libc::EIO);
            } else {
                reply.ok();
            }
        } else {
            error!(self.logger, "fsync error: no entry");
            reply.error(libc::ENOENT);
        }
    }

    fn getxattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        debug!(self.logger, "getxattr");
        self.metrics.io_operations_getxattr.increment();

        reply.error(libc::ENOSYS);
    }

    fn destroy(&mut self, _req: &Request) {
        debug!(self.logger, "destroy");
        self.metrics.io_operations_destroy.increment();
    }

    fn forget(&mut self, _req: &Request, _ino: u64, _nlookup: u64) {
        debug!(self.logger, "forget");
        self.metrics.io_operations_forget.increment();
    }

    fn readlink(&mut self, _req: &Request, _ino: u64, reply: ReplyData) {
        debug!(self.logger, "readlink");
        self.metrics.io_operations_readlink.increment();
        reply.error(libc::ENOSYS);
    }

    fn mknod(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        debug!(self.logger, "mknod");
        self.metrics.io_operations_mknod.increment();
        reply.error(libc::ENOSYS);
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        reply: ReplyEntry,
    ) {
        debug!(self.logger, "mkdir");
        self.metrics.io_operations_mkdir.increment();
        reply.error(libc::ENOSYS);
    }

    fn unlink(&mut self, _req: &Request, _parent: u64, _name: &OsStr, reply: ReplyEmpty) {
        debug!(self.logger, "unlink");
        self.metrics.io_operations_unlink.increment();
        reply.error(libc::ENOSYS);
    }

    fn rmdir(&mut self, _req: &Request, _parent: u64, _name: &OsStr, reply: ReplyEmpty) {
        debug!(self.logger, "rmdir");
        self.metrics.io_operations_rmdir.increment();
        reply.error(libc::ENOSYS);
    }

    fn symlink(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _link: &Path,
        reply: ReplyEntry,
    ) {
        debug!(self.logger, "symlink");
        self.metrics.io_operations_symlink.increment();
        reply.error(libc::ENOSYS);
    }

    fn rename(
        &mut self,
        _req: &Request,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        reply: ReplyEmpty,
    ) {
        debug!(self.logger, "rename");
        self.metrics.io_operations_rename.increment();
        reply.error(libc::ENOSYS);
    }

    fn link(
        &mut self,
        _req: &Request,
        _ino: u64,
        _newparent: u64,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        debug!(self.logger, "link");
        self.metrics.io_operations_link.increment();
        reply.error(libc::ENOSYS);
    }

    fn opendir(&mut self, _req: &Request, ino: u64, _flags: u32, reply: ReplyOpen) {
        debug!(self.logger, "opendir: ino: {}", ino);
        self.metrics.io_operations_opendir.increment();

        reply.opened(0, 0);
    }

    fn releasedir(&mut self, _req: &Request, _ino: u64, _fh: u64, _flags: u32, reply: ReplyEmpty) {
        debug!(self.logger, "releasedir");
        self.metrics.io_operations_releasedir.increment();
        reply.ok();
    }

    fn fsyncdir(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        debug!(self.logger, "fsyncdir");
        self.metrics.io_operations_fsyncdir.increment();
        reply.error(libc::ENOSYS);
    }

    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        // debug!(self.logger, "statfs");
        reply.statfs(0, 0, 0, 0, 0, 512, 255, 0);
        self.metrics.io_operations_statfs.increment();
    }

    fn setxattr(
        &mut self,
        _req: &Request,
        _ino: u64,
        _name: &OsStr,
        _value: &[u8],
        _flags: u32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        debug!(self.logger, "setxattr");
        self.metrics.io_operations_setxattr.increment();
        reply.error(libc::ENOSYS);
    }

    fn listxattr(&mut self, _req: &Request, _ino: u64, _size: u32, reply: ReplyXattr) {
        debug!(self.logger, "listxattr");
        self.metrics.io_operations_listxattr.increment();
        reply.error(libc::ENOSYS);
    }

    fn removexattr(&mut self, _req: &Request, _ino: u64, _name: &OsStr, reply: ReplyEmpty) {
        debug!(self.logger, "removexattr");
        self.metrics.io_operations_removexattr.increment();
        reply.error(libc::ENOSYS);
    }

    fn access(&mut self, _req: &Request, _ino: u64, _mask: u32, reply: ReplyEmpty) {
        debug!(self.logger, "access");
        self.metrics.io_operations_access.increment();
        reply.error(libc::ENOSYS);
    }

    fn create(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        flags: u32,
        reply: ReplyCreate,
    ) {
        debug!(self.logger, "create: parent: {}, name: {:?}", parent, name);
        self.metrics.io_operations_create.increment();

        match Mizumochi::create(self, req, parent, name, mode, flags) {
            Ok((attr, fh)) => reply.created(&TTL, &attr, 0, fh, 0),
            Err(error) => {
                error!(self.logger, "init error: {}", error.description());
                reply.error(libc::EIO)
            }
        }
    }

    fn getlk(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        _start: u64,
        _end: u64,
        _typ: u32,
        _pid: u32,
        reply: ReplyLock,
    ) {
        debug!(self.logger, "getlk");
        self.metrics.io_operations_getlk.increment();
        reply.error(libc::ENOSYS);
    }

    fn setlk(
        &mut self,
        _req: &Request,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        _start: u64,
        _end: u64,
        _typ: u32,
        _pid: u32,
        _sleep: bool,
        reply: ReplyEmpty,
    ) {
        debug!(self.logger, "setlk");
        self.metrics.io_operations_setlk.increment();
        reply.error(libc::ENOSYS);
    }

    fn bmap(&mut self, _req: &Request, _ino: u64, _blocksize: u32, _idx: u64, reply: ReplyBmap) {
        debug!(self.logger, "bmap");
        self.metrics.io_operations_bmap.increment();
        reply.error(libc::ENOSYS);
    }
}

fn timespec_from(st: &std::time::SystemTime) -> Timespec {
    if let Ok(dur_since_epoch) = st.duration_since(std::time::UNIX_EPOCH) {
        Timespec::new(
            dur_since_epoch.as_secs() as i64,
            dur_since_epoch.subsec_nanos() as i32,
        )
    } else {
        Timespec::new(0, 0)
    }
}

fn fetch_fileattr(ino: u64, filepath: &Path) -> Result<FileAttr, io::Error> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(filepath)?;
    let mode = metadata.permissions().mode();
    let kind = mode & libc::S_IFMT as u32;

    let default_timespec = Timespec::new(0, 0);
    let mut attr: FileAttr = unsafe { mem::zeroed() };
    attr.ino = ino;
    attr.size = metadata.len();
    attr.atime = metadata
        .accessed()
        .map(|time| timespec_from(&time))
        .unwrap_or(default_timespec);
    attr.mtime = metadata
        .modified()
        .map(|time| timespec_from(&time))
        .unwrap_or(default_timespec);
    attr.ctime = metadata
        .created()
        .map(|time| timespec_from(&time))
        .unwrap_or(default_timespec);
    attr.kind = if kind == libc::S_IFREG as u32 {
        FileType::RegularFile
    } else if kind == libc::S_IFDIR as u32 {
        FileType::Directory
    } else if kind == libc::S_IFIFO as u32 {
        FileType::NamedPipe
    } else if kind == libc::S_IFCHR as u32 {
        FileType::CharDevice
    } else if kind == libc::S_IFBLK as u32 {
        FileType::BlockDevice
    } else if kind == libc::S_IFLNK as u32 {
        FileType::Symlink
    } else if kind == libc::S_IFSOCK as u32 {
        FileType::Socket
    } else {
        return Err(io::Error::new(io::ErrorKind::Other, "unknown kind"));
    };
    attr.perm = (mode & (libc::S_IRWXU | libc::S_IRWXG | libc::S_IRWXO) as u32) as u16;
    attr.uid = metadata.uid();
    attr.gid = metadata.gid();
    attr.nlink = metadata.nlink() as u32;

    Ok(attr)
}

/// `request_bps` means request Byte per seconds (not bit).
/// `count_byte` is the number of read/written bytes.
/// `elapsed_ms` is the elapsed time in milliseconds to read/write data.
fn compute_sleep_duration_to_adjust_speed(
    request_bps: usize,
    count_byte: usize,
    elapsed_ms: u64,
) -> Duration {
    if request_bps == 0 {
        panic!("The given request bps is zero.");
    }

    let expect_sec = count_byte as f64 / request_bps as f64;
    let expect_ms = (expect_sec * 1000.0).round() as u64;
    let wait_ms = expect_ms.saturating_sub(elapsed_ms);

    Duration::from_millis(wait_ms as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_sleep_duration_to_adjust_speed() {
        assert_eq!(
            Duration::from_millis(0),
            compute_sleep_duration_to_adjust_speed(1024, 0, 0)
        );
        assert_eq!(
            Duration::from_millis(0),
            compute_sleep_duration_to_adjust_speed(1024, 0, 512)
        );
        assert_eq!(
            Duration::from_millis(500),
            compute_sleep_duration_to_adjust_speed(1024, 512, 0)
        );
        assert_eq!(
            Duration::from_millis(1000),
            compute_sleep_duration_to_adjust_speed(1024, 1024, 0)
        );
        assert_eq!(
            Duration::from_millis(2000),
            compute_sleep_duration_to_adjust_speed(1024, 2048, 0)
        );
        assert_eq!(
            Duration::from_millis(0),
            compute_sleep_duration_to_adjust_speed(1024, 256, 512)
        );
        assert_eq!(
            Duration::from_millis(0),
            compute_sleep_duration_to_adjust_speed(1024, 512, 512)
        );
        assert_eq!(
            Duration::from_millis(0),
            compute_sleep_duration_to_adjust_speed(1024, 512, 1000)
        );
    }
}
