use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub duration: Duration,
    pub frequency: Duration,
    pub operations: Vec<Operation>,
    pub speed: Speed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Speed {
    Bps(usize),
    PassThrough,
}
impl fmt::Display for Speed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Speed::Bps(bps) => write!(f, "{}B/s", bps),
            Speed::PassThrough => write!(f, "PassThrough"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Operation {
    Read,
    Write,
}
impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Operation::Read => write!(f, "Read"),
            Operation::Write => write!(f, "Write"),
        }
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            duration: Duration::from_secs(10 * 60),
            frequency: Duration::from_secs(30 * 60),
            operations: vec![Operation::Read, Operation::Write],
            speed: Speed::PassThrough,
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let d = self.duration.as_secs();
        let f = self.frequency.as_secs();
        let ops = self
            .operations
            .iter()
            .fold(Vec::new(), |mut acc, x| {
                acc.push(format!("{}", x).to_string());
                acc
            })
            .join(":");
        write!(
            fmt,
            "Config {{Duration: {}sec, Frequency: {}sec, Operations: {}, Speed: {}}}",
            d, f, ops, self.speed
        )
    }
}
