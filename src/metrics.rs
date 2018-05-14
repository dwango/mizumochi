use prometrics::metrics::{Counter, MetricBuilder};

#[derive(Debug)]
pub struct Metrics {
    pub io_operations_lookup: Counter,
    pub io_operations_getattr: Counter,
    pub io_operations_readdir: Counter,
    pub io_operations_read: Counter,
    pub io_operations_setattr: Counter,
    pub io_operations_write: Counter,
    pub io_operations_open: Counter,
    pub io_operations_flush: Counter,
    pub io_operations_release: Counter,
    pub io_operations_fsync: Counter,
    pub io_operations_getxattr: Counter,
    pub io_operations_destroy: Counter,
    pub io_operations_forget: Counter,
    pub io_operations_readlink: Counter,
    pub io_operations_unlink: Counter,
    pub io_operations_symlink: Counter,
    pub io_operations_link: Counter,
    pub io_operations_mknod: Counter,
    pub io_operations_mkdir: Counter,
    pub io_operations_rmdir: Counter,
    pub io_operations_rename: Counter,
    pub io_operations_opendir: Counter,
    pub io_operations_releasedir: Counter,
    pub io_operations_fsyncdir: Counter,
    pub io_operations_statfs: Counter,
    pub io_operations_setxattr: Counter,
    pub io_operations_listxattr: Counter,
    pub io_operations_removexattr: Counter,
    pub io_operations_access: Counter,
    pub io_operations_create: Counter,
    pub io_operations_getlk: Counter,
    pub io_operations_setlk: Counter,
    pub io_operations_bmap: Counter,
    pub speed_limit_enabled: Counter,
    pub speed_limit_disabled: Counter,
}
impl Metrics {
    pub fn new() -> Self {
        let mut builder = MetricBuilder::new();
        builder.namespace("mizumochi");
        let build_io_operations_metric = |name| {
            builder
                .counter("io_operations_total")
                .label("operation", name)
                .help("Number of I/O operations")
                .finish()
                .expect("Never fails")
        };
        Metrics {
            io_operations_lookup: build_io_operations_metric("lookup"),
            io_operations_getattr: build_io_operations_metric("getattr"),
            io_operations_readdir: build_io_operations_metric("readdir"),
            io_operations_read: build_io_operations_metric("read"),
            io_operations_setattr: build_io_operations_metric("setattr"),
            io_operations_write: build_io_operations_metric("write"),
            io_operations_open: build_io_operations_metric("open"),
            io_operations_flush: build_io_operations_metric("flush"),
            io_operations_release: build_io_operations_metric("release"),
            io_operations_fsync: build_io_operations_metric("fsync"),
            io_operations_getxattr: build_io_operations_metric("getxattr"),
            io_operations_destroy: build_io_operations_metric("destroy"),
            io_operations_forget: build_io_operations_metric("forget"),
            io_operations_readlink: build_io_operations_metric("read_link"),
            io_operations_unlink: build_io_operations_metric("unlink"),
            io_operations_symlink: build_io_operations_metric("symlink"),
            io_operations_link: build_io_operations_metric("link"),
            io_operations_mknod: build_io_operations_metric("mknod"),
            io_operations_mkdir: build_io_operations_metric("mkdir"),
            io_operations_rmdir: build_io_operations_metric("rmdir"),
            io_operations_rename: build_io_operations_metric("rename"),
            io_operations_opendir: build_io_operations_metric("opendir"),
            io_operations_releasedir: build_io_operations_metric("releasedir"),
            io_operations_fsyncdir: build_io_operations_metric("fsyncdir"),
            io_operations_statfs: build_io_operations_metric("statfs"),
            io_operations_setxattr: build_io_operations_metric("setxattr"),
            io_operations_listxattr: build_io_operations_metric("listxattr"),
            io_operations_removexattr: build_io_operations_metric("removexattr"),
            io_operations_access: build_io_operations_metric("access"),
            io_operations_create: build_io_operations_metric("create"),
            io_operations_getlk: build_io_operations_metric("getlk"),
            io_operations_setlk: build_io_operations_metric("setlk"),
            io_operations_bmap: build_io_operations_metric("bmap"),
            speed_limit_enabled: builder
                .counter("speed_limit_enabled_total")
                .help("Number of times speed limit has been enabled")
                .finish()
                .expect("Never fails"),
            speed_limit_disabled: builder
                .counter("speed_limit_disabled_total")
                .help("Number of times speed limit has been disabled")
                .finish()
                .expect("Never fails"),
        }
    }
}
