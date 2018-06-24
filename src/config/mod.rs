use std::fmt;

pub use self::condition::Condition;
pub use self::operation::Operation;
pub use self::speed::Speed;

mod condition;
mod operation;
mod speed;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub speed: Speed,
    pub operations: Vec<Operation>,
    pub condition: Condition,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            speed: Speed::PassThrough,
            operations: vec![Operation::Read, Operation::Write],
            condition: Condition::default_periodic(),
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let ops = self
            .operations
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(":");
        write!(
            fmt,
            "config {{speed: {}, operations: {}, condition: {:?}}}",
            ops, self.speed, self.condition
        )
    }
}
