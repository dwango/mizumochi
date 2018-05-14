extern crate atomic_immut;
extern crate bytecodec;
#[macro_use]
extern crate clap;
extern crate fibers;
extern crate fibers_http_server;
extern crate fuse;
extern crate futures;
extern crate httpcodec;
extern crate libc;
extern crate prometrics;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate time;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_term;

mod config;
mod http;
mod metrics;
mod mizumochi;

use atomic_immut::AtomicImmut;
use clap::Arg;
use config::{Config, Speed};
use mizumochi::Mizumochi;
use slog::{Drain, Level};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn main() -> Result<(), Box<std::error::Error>> {
    let matches = app_from_crate!()
        .arg(
            Arg::with_name("SPEED_BPS")
                .short("s")
                .long("speed_bps")
                .value_name("BytePerSecond")
                .help("Sets byte per second to limit file operations")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("DURATION")
                .short("d")
                .long("duration")
                .value_name("Duration")
                .help("Sets period during the operations are unstable")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("FREQUENCY")
                .short("f")
                .long("frequency")
                .value_name("Frequency")
                .help("Sets frequency of making operations unstable")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("HTTP_PORT")
                .short("p")
                .long("http-port")
                .help("Sets HTTP server listen portis listening")
                .takes_value(true)
                .default_value("33133"),
        )
        .arg(
            Arg::with_name("SRC_FILE")
                .help("Sets the source file to use")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("DST_FILE")
                .help("Sets the destination file to use")
                .required(true)
                .index(2),
        )
        .get_matches();

    let src = matches.value_of("SRC_FILE").unwrap();
    let dst = matches.value_of("DST_FILE").unwrap();
    let http_port: u16 = matches.value_of("HTTP_PORT").unwrap().parse()?;

    let src = Path::new(src).canonicalize()?;
    let dst = Path::new(dst);
    let mountpoint = dst.parent().ok_or("cannot get mountpoint")?;

    let mut config: Config = Default::default();

    // Override the config if there are given options.
    if let Some(speed_bps) = matches.value_of("SPEED_BPS") {
        config.speed = Speed::Bps(speed_bps.parse()?);
    }

    if let Some(duration) = matches.value_of("DURATION") {
        let secs = parse_time(String::from(duration))?;
        config.duration = Duration::from_secs(secs);
    }

    if let Some(frequency) = matches.value_of("FREQUENCY") {
        let secs = parse_time(String::from(frequency))?;
        config.frequency = Duration::from_secs(secs);
    }

    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();
    let drain = slog::Fuse::new(slog::LevelFilter::new(drain, Level::Info));
    let logger = slog::Logger::root(drain, o!());

    info!(logger, "src: {}", src.to_str().expect("src is invalid"));
    info!(logger, "dst: {}", dst.to_str().expect("dst is invalid"));
    info!(logger, "config: {}", config);

    let config = Arc::new(AtomicImmut::new(config));
    http::start_server(logger.clone(), http_port, Arc::clone(&config))?;

    let m = Mizumochi::new(
        logger.clone(),
        mountpoint.into(),
        src.into(),
        dst.into(),
        config,
    );

    if let Err(error) = m.mount() {
        error!(logger, "{}", error);
        Err(Box::new(error))
    } else {
        Ok(())
    }
}

fn parse_time(mut input: String) -> Result<u64, Box<std::error::Error>> {
    let suffix = input.pop().ok_or(std::fmt::Error)?;
    let t: u64 = input.parse()?;

    use std::io::{Error, ErrorKind};
    match suffix {
        's' => Ok(t),
        'm' => Ok(t * 60),
        'h' => Ok(t * 60 * 60),
        _ => Err(Box::new(Error::new(
            ErrorKind::Other,
            "time suffix accepts s, m or h",
        ))),
    }
}
