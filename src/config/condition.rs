use state::State;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Condition {
    Periodic {
        duration: Duration,
        frequency: Duration,
    },
    Always(State),
}

impl Condition {
    pub fn default_periodic() -> Condition {
        Condition::Periodic {
            duration: Duration::from_secs(10 * 60),
            frequency: Duration::from_secs(30 * 60),
        }
    }
}
