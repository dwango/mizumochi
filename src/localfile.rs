use std::path::PathBuf;

pub type Inode = u64;

#[derive(Debug, Clone)]
pub enum LocalFile {
    RegularFile(PathBuf),
    // Note that the `PathBuf` in Vec<(Inode, PathBuf)> refers filename (not filepath).
    Directory(PathBuf, Option<Vec<(Inode, PathBuf)>>),
}
