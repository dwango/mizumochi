# mizumochi
[![Crates.io](https://img.shields.io/crates/v/mizumochi.svg)](https://crates.io/crates/mizumochi) 
[![Crates.io](https://img.shields.io/crates/d/mizumochi.svg)](https://crates.io/crates/mizumochi) 
[![License: Apache](https://img.shields.io/badge/License-Apache%202.0-red.svg)](LICENSE-APACHE)
OR
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE-MIT)

mizumochi is a tool to simulate unstable disk I/O for testing stability/robustness of system.  
The word unstable here means read/write speed is slowdown.

We assume mizumochi works on develop environment with target system.

[zargony/rust-fuse](https://github.com/zargony/rust-fuse) are used to maps actual files in the given directory to files on the mountpoint.
*Note that some FUSE callbacks (e.g., link) are not implemented yet. (work in progress)*


## Install
You have to install [OSXFUSE](http://osxfuse.github.io) for macOS or [FUSE](http://fuse.sourceforge.net) for Linux before installing mizumochi.

```console
cargo install mizumochi
```


## Features
- Mode
    + Periodic
        * The stable/unstable is toggled periodically.
- Interfaces
    + Command line interface (CLI)
        * CLI is primary interface.
        * Refers `mizumochi --help` in details.
    + HTTP API
        * There are some TODOs.
        * The config (e.g., speed, condition to switch stable/unstable) can be modified on runtime via this interface.

## Examples
```console
# Emulate files in `real_dir` at `emulated_dir` and the read/write speed is slowdown every 30 minutes for 10 minutes.
# Slowdown happens in `emulated_dir`.
mizumochi /tmp/real_dir/ /tmp/emulated_dir/ --speed 1024KBps periodic --duration 10m --frequency 30m
```


## License
Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.


## Contribution
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
