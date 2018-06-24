use std::fmt;

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
