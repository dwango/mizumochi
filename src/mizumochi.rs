// FIXME: Refactor error
use atomic_immut::AtomicImmut;
use config::{Config, Speed};
use fuse::{self, *};
use libc;
use metrics::Metrics;
use slog::Logger;
use std;
use std::collections::HashMap;
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
use std::time::Instant;
use time::{PreciseTime, Timespec};

type FileHandler = u64;
type Inode = u64;

const TTL: Timespec = Timespec { sec: 1, nsec: 0 };
const ROOT_DIR_INO: u64 = 1;

pub struct Mizumochi {
    logger: Logger,

    config: Arc<AtomicImmut<Config>>,

    is_unstable: bool,
    current_mode_begin_time: Instant,

    // FIXME: use simple allocator.
    fh_count: FileHandler,
    ino_count: Inode,
    fh_map: HashMap<FileHandler, (Inode, File)>,
    ino_map: HashMap<Inode, PathBuf>,
    name_map: HashMap<(Inode, PathBuf), Inode>,

    mountpoint: PathBuf,
    src_file_path: PathBuf,
    dst_file_path: PathBuf,
    src_dir_path: String,

    metrics: Metrics,
}

impl Mizumochi {
    pub fn new(
        logger: Logger,
        mountpoint: PathBuf,
        src: PathBuf,
        dst: PathBuf,
        config: Arc<AtomicImmut<Config>>,
    ) -> Mizumochi {
        Mizumochi {
            logger,

            config,

            is_unstable: false,
            current_mode_begin_time: Instant::now(),

            fh_count: 1,
            ino_count: 2,
            fh_map: HashMap::new(),
            ino_map: HashMap::new(),
            name_map: HashMap::new(),

            mountpoint,
            src_file_path: src,
            dst_file_path: dst,
            src_dir_path: String::new(),

            metrics: Metrics::new(),
        }
    }

    pub fn mount(self) -> Result<(), io::Error> {
        let mountpoint = self.mountpoint.clone();
        fuse::mount(self, &mountpoint, &[])
    }

    fn toggle_mode_if_necessary(&mut self) -> bool {
        let config = self.config.load(); // Takes snapshot
        let (next_mode, d) = toggle_mode_if_necessary(
            self.is_unstable,
            config.duration,
            config.frequency,
            self.current_mode_begin_time.elapsed().as_secs(),
        );
        self.current_mode_begin_time += d;
        match (self.is_unstable, next_mode) {
            (false, true) => {
                self.is_unstable = true;
                self.metrics.speed_limit_enabled.increment();
                info!(self.logger, "--- Enable unstable mode ---")
            }
            (true, false) => {
                self.is_unstable = false;
                self.metrics.speed_limit_disabled.increment();
                info!(self.logger, "--- Disable unstable mode ---")
            }
            _ => {}
        }

        self.is_unstable
    }

    fn lookup(&self, parent: u64, name: &OsStr) -> Result<FileAttr, c_int> {
        let key = (parent, name.into());
        let ino = self.name_map.get(&key).ok_or(libc::ENOENT)?;
        let path = self.ino_map.get(&ino).ok_or(libc::ENOENT)?;

        fetch_fileattr(*ino, path).or(Err(libc::EIO))
    }

    fn read(&mut self, fh: u64, buffer: &mut [u8], offset: i64, size: u32) -> Result<usize, c_int> {
        let logger = &self.logger;
        let (_, f) = self.fh_map.get_mut(&fh).ok_or(libc::ENOENT)?;

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
        let (_, f) = self.fh_map.get_mut(&fh).ok_or(libc::ENOENT)?;
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
}

impl Filesystem for Mizumochi {
    fn init(&mut self, _req: &Request) -> Result<(), c_int> {
        info!(self.logger, "Initialize");

        // Initialize timers for switching stable/unstable.
        let now = Instant::now();
        self.current_mode_begin_time = now;
        info!(self.logger, "is_unstable: {}", self.is_unstable);

        // Store the filepaths.
        let ino = self.ino_count;
        self.ino_count += 1;
        self.ino_map.insert(ino, self.src_file_path.clone());

        self.src_dir_path = self
            .src_file_path
            .parent()
            .ok_or(libc::EIO)?
            .to_str()
            .ok_or(libc::EIO)?
            .to_string();
        self.ino_map
            .insert(ROOT_DIR_INO, self.src_dir_path.clone().into());

        let filename = self.dst_file_path.file_name().ok_or(libc::EIO)?;
        self.name_map.insert((ROOT_DIR_INO, filename.into()), ino);

        Ok(())
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        debug!(self.logger, "lookup: parent: {}, name: {:?}", parent, name);
        self.metrics.io_operations_lookup.increment();

        match Mizumochi::lookup(self, parent, name) {
            Ok(ref attr) => reply.entry(&TTL, attr, 0),
            Err(ecode) => {
                error!(
                    self.logger,
                    "lookup error: parent: {}, name: {:?}", parent, name
                );
                reply.error(ecode)
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        debug!(self.logger, "getattr: ino: {:?}", ino);
        self.metrics.io_operations_getattr.increment();

        if let Some(path) = self.ino_map.get(&ino) {
            match fetch_fileattr(ino, path) {
                Ok(attr) => reply.attr(&TTL, &attr),
                Err(error) => {
                    error!(self.logger, "getattr error: {}", error);
                    reply.error(libc::EIO)
                }
            }
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
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

        if ino == ROOT_DIR_INO {
            // Support one root currently.
            if offset == 0 {
                // reply.add(ino, offset, kind: FileType, name);
                reply.add(1, 0, FileType::Directory, ".");
                reply.add(1, 1, FileType::Directory, "..");
                for ((_, path), ino) in self
                    .name_map
                    .iter()
                    .filter(|((ino, _), _)| *ino == ROOT_DIR_INO)
                {
                    reply.add(*ino, *ino as i64, FileType::RegularFile, path);
                }
            }
            reply.ok();
        } else {
            reply.error(libc::ENOENT);
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

                if self.toggle_mode_if_necessary() {
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

        // TODO: apply the argument to the dst file attribute (self.dst_attr).
        match self.ino_map.get(&ino) {
            Some(path) => match fetch_fileattr(ino, path) {
                Ok(ref attr) => reply.attr(&TTL, attr),
                Err(_) => reply.error(libc::EIO),
            },
            None => reply.error(libc::ENOENT),
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

                if self.toggle_mode_if_necessary() {
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

        if let Some(path) = self.ino_map.get(&ino) {
            info!(self.logger, " -> {:?}", path);
            let mut options = fs::OpenOptions::new();
            options.read(true).write(true).create(false);

            match options.open(path) {
                Ok(f) => {
                    let fh = self.fh_count;
                    self.fh_count += 1;
                    self.fh_map.insert(fh, (ino, f));

                    reply.opened(fh, 0);
                }
                Err(error) => {
                    error!(self.logger, "open error: {}", error);
                    reply.error(libc::EIO)
                }
            }
        } else {
            reply.error(libc::ENOENT)
        }
    }

    fn flush(&mut self, _req: &Request, ino: u64, fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        debug!(self.logger, "flush: ino: {}, fh: {}", ino, fh);
        self.metrics.io_operations_flush.increment();

        if let Some((_, f)) = self.fh_map.get_mut(&fh) {
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

        if let Some((_, f)) = self.fh_map.remove(&fh) {
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

        if let Some((_, f)) = self.fh_map.get(&fh) {
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

    fn opendir(&mut self, _req: &Request, _ino: u64, _flags: u32, reply: ReplyOpen) {
        debug!(self.logger, "opendir");
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
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _flags: u32,
        reply: ReplyCreate,
    ) {
        debug!(self.logger, "create");
        self.metrics.io_operations_create.increment();

        if parent != ROOT_DIR_INO {
            // Current support is only the root directory.
            reply.error(libc::ENOSYS);
            return;
        }

        let name = if let Some(name) = name.to_str() {
            name
        } else {
            reply.error(libc::EIO);
            return;
        };

        let path = self.src_dir_path.clone() + "/" + name;
        let path = Path::new(&path);

        match File::create(path) {
            Ok(file) => {
                let ino = self.ino_count;
                self.ino_count += 1;

                match fetch_fileattr(ino, path) {
                    Ok(attr) => {
                        let fh = self.fh_count;
                        self.fh_count += 1;

                        reply.created(&TTL, &attr, 0, fh, 0);

                        let path = path.to_path_buf();
                        self.fh_map.insert(fh, (ino, file));
                        self.ino_map.insert(ino, path.clone());
                        self.name_map.insert((ROOT_DIR_INO, name.into()), ino);
                    }
                    Err(_) => {
                        self.ino_count -= 1;
                        reply.error(libc::EIO);
                    }
                }
            }
            Err(_) => reply.error(libc::EIO),
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

fn toggle_mode_if_necessary(
    is_unstable: bool,
    duration: Duration,
    frequency: Duration,
    elapsed: u64,
) -> (bool, Duration) {
    let frequency = frequency.as_secs();
    let duration = duration.as_secs();
    let one_term = frequency + duration;

    let cnt = elapsed / one_term;
    let elapsed = elapsed % one_term;

    let t = if !is_unstable { frequency } else { duration };

    if t < elapsed {
        // Toggle the mode if the elapsed time exceeds the current mode duration.
        (!is_unstable, Duration::from_secs(cnt * one_term + t))
    } else {
        // Keep
        (is_unstable, Duration::from_secs(cnt * one_term))
    }
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

    #[test]
    fn test_toggle_mode() {
        let is_unstable = true;
        let duration = Duration::from_secs(10);
        let frequency = Duration::from_secs(60);

        let mut elapsed = 0;

        // Keep.
        let (is_unstable, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(true, is_unstable);
        assert_eq!(0, elapsed);

        // Change it to stable.
        elapsed += 11;
        let (is_unstable, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(false, is_unstable);
        assert_eq!(1, elapsed);

        // Change it to unstable.
        elapsed += 60;
        let (is_unstable, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(true, is_unstable);
        assert_eq!(1, elapsed);

        // Keep unstable.
        elapsed += 10 + 60;
        let (is_unstable, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(true, is_unstable);
        assert_eq!(1, elapsed);

        // Change it to stable.
        elapsed += 10;
        let (is_unstable, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(false, is_unstable);
        assert_eq!(1, elapsed);
    }

    #[test]
    fn test_toggle_mode_stable_to_unstable() {
        let is_unstable = false;
        let duration = Duration::from_secs(10);
        let frequency = Duration::from_secs(60);

        let mut elapsed = 60 + 1;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(true, f);
        assert_eq!(1, elapsed);

        let mut elapsed = 60 + 10 + 60 + 1;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(true, f);
        assert_eq!(1, elapsed);
    }

    #[test]
    fn test_toggle_mode_unstable_to_stable() {
        let is_unstable = true;
        let duration = Duration::from_secs(10);
        let frequency = Duration::from_secs(60);

        let mut elapsed = 10 + 1;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(false, f);
        assert_eq!(1, elapsed);

        let mut elapsed = 10 + 60 + 10 + 1;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(false, f);
        assert_eq!(1, elapsed);
    }

    #[test]
    fn test_toggle_mode_keep_unstable() {
        let is_unstable = true;
        let duration = Duration::from_secs(10);
        let frequency = Duration::from_secs(60);

        let mut elapsed = 1;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(true, f);
        assert_eq!(1, elapsed);

        let mut elapsed = 8;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(true, f);
        assert_eq!(8, elapsed);

        let mut elapsed = 10 + 60 + 1;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(true, f);
        assert_eq!(1, elapsed);
    }

    #[test]
    fn test_toggle_mode_keep_stable() {
        let is_unstable = false;
        let duration = Duration::from_secs(10);
        let frequency = Duration::from_secs(60);

        let mut elapsed = 1;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(false, f);
        assert_eq!(1, elapsed);

        let mut elapsed = 8;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(false, f);
        assert_eq!(8, elapsed);

        let mut elapsed = 60 + 10 + 60 + 10 + 1;
        let (f, d) = toggle_mode_if_necessary(is_unstable, duration, frequency, elapsed);
        elapsed -= d.as_secs();
        assert_eq!(false, f);
        assert_eq!(1, elapsed);
    }
}
