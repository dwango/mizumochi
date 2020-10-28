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
mod localfile;
mod metrics;
mod mizumochi;
mod state;

use crate::config::*;
use crate::mizumochi::Mizumochi;
use atomic_immut::AtomicImmut;
use clap::{Arg, SubCommand};
use slog::{Drain, Level};
use std::sync::Arc;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = app_from_crate!()
        .arg(
            Arg::with_name("SPEED")
                .short("s")
                .long("speed")
                .value_name("BytePerSecond")
                .help("Sets byte per second to limit file operations")
                .long_help("you can put suffixes (KBps, MBps, GBps) at the tail (examples: 1024Bps, 4096KBps, 5Mbps)\nthe default is Bps")
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
            Arg::with_name("ORIGINAL_DIR")
                .help("Sets a directory has original files")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("MOUNTPOINT")
                .help("Mountpoint directory")
                .required(true)
                .index(2),
        )
         .subcommand(
             SubCommand::with_name("periodic")
                 .about("Stable/unstable mode toggles periodically under this condition")
                 .arg(
                     Arg::with_name("DURATION")
                         .short("d")
                         .long("duration")
                         .takes_value(true)
                         .default_value("30m")
                         .required(true)
                         .help("Sets period during the operations are unstable"),
                 )
                 .arg(
                     Arg::with_name("FREQUENCY")
                         .short("f")
                         .long("frequency")
                         .takes_value(true)
                         .default_value("60m")
                         .required(true)
                         .help("Sets frequency of making operations unstable"),
                 ))
        .get_matches();

    let original_dir = matches.value_of("ORIGINAL_DIR").unwrap();
    let mountpoint = matches.value_of("MOUNTPOINT").unwrap();
    let http_port: u16 = matches.value_of("HTTP_PORT").unwrap().parse()?;

    let mut config: Config = Default::default();

    // Override the config if there are given options.
    if let Some(speed) = matches.value_of("SPEED") {
        config.speed = speed.parse()?;
    }

    if let Some(matches) = matches.subcommand_matches("periodic") {
        let mut p = config::Condition::default_periodic();

        if let Some(duration) = matches.value_of("DURATION") {
            let secs = parse_time(String::from(duration))?;

            if let Condition::Periodic {
                ref mut duration, ..
            } = p
            {
                *duration = Duration::from_secs(secs);
            }
        }

        if let Some(frequency) = matches.value_of("FREQUENCY") {
            let secs = parse_time(String::from(frequency))?;

            if let Condition::Periodic {
                ref mut frequency, ..
            } = p
            {
                *frequency = Duration::from_secs(secs);
            }
        }

        config.condition = p;
    }

    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();
    let drain = slog::Fuse::new(slog::LevelFilter::new(drain, Level::Info));
    let logger = slog::Logger::root(drain, o!());

    info!(logger, "original directory: {}", original_dir);
    info!(logger, "mountpoint: {}", mountpoint);
    info!(logger, "config: {}", config);

    let config = Arc::new(AtomicImmut::new(config));
    http::start_server(logger.clone(), http_port, Arc::clone(&config))?;

    let m = Mizumochi::new(
        logger.clone(),
        original_dir.into(),
        mountpoint.into(),
        config,
    );

    if let Err(error) = m.mount() {
        error!(logger, "{}", error);
        Err(Box::new(error))
    } else {
        Ok(())
    }
}

fn parse_time(mut input: String) -> Result<u64, Box<dyn std::error::Error>> {
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
