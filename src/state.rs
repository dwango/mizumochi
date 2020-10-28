use crate::config::{Condition, Operation};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Stable,
    Unstable,
}

/// `StateManager` stores information for a condition to toggle stable/unstable.
pub struct StateManager {
    // Keep current condition to detect changing the condition.
    // NOTE: Find more clever implementation.
    condition: Condition,
    state: State,
    current_state_begin_time: Instant,
}

impl StateManager {
    pub fn new(condition: Condition) -> StateManager {
        StateManager {
            condition,
            state: State::Stable,
            current_state_begin_time: Instant::now(),
        }
    }

    pub fn init(&mut self) {
        match self.condition {
            Condition::Always(ref s) => self.state = s.clone(),
            _ => self.state = State::Stable,
        }

        self.current_state_begin_time = Instant::now();
    }

    pub fn change_condition(&mut self, c: &Condition) {
        self.condition = c.clone();
        self.init()
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn on_operated_after(&mut self, _: Operation, cond: &Condition) -> Result<&State, String> {
        if self.condition != *cond {
            self.change_condition(cond);
        }

        use crate::Condition::*;
        match self.condition {
            Periodic {
                ref duration,
                ref frequency,
            } => {
                let elapsed = self.current_state_begin_time.elapsed().as_secs();
                let (next_mode, d) = toggle_mode_if_necessary(
                    self.state == State::Unstable,
                    duration,
                    frequency,
                    elapsed,
                );
                self.current_state_begin_time += d;

                match (self.state == State::Unstable, next_mode) {
                    (false, true) => {
                        self.state = State::Unstable;
                    }
                    (true, false) => {
                        self.state = State::Stable;
                    }
                    _ => {}
                }
            }
            Always(_) => {
                // Keep the current state,
            }
        }

        Ok(&self.state)
    }
}

fn toggle_mode_if_necessary(
    is_unstable: bool,
    duration: &Duration,
    frequency: &Duration,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use atomic_immut::AtomicImmut;
    use std::sync::Arc;

    struct TestFileSystem {
        config: Arc<AtomicImmut<Config>>,
        stat: StateManager,
    }

    #[test]
    fn test_state_manager() {
        let mut config = Config::default();
        config.condition = Condition::Periodic {
            duration: Duration::from_secs(10 * 60),
            frequency: Duration::from_secs(30 * 60),
        };

        let stat = StateManager::new(config.condition.clone());

        let mut fs = TestFileSystem {
            config: Arc::new(AtomicImmut::new(config)),
            stat,
        };

        fs.stat.init();

        // Change the time for test.
        fs.stat.current_state_begin_time = Instant::now() - Duration::from_secs(5 * 60);

        // The state is kept.
        let cond = &fs.config.load().condition;
        assert_eq!(
            Ok(&State::Stable),
            fs.stat.on_operated_after(Operation::Read, cond)
        );

        // Change the time for test.
        fs.stat.current_state_begin_time = Instant::now() - Duration::from_secs(35 * 60);

        // The state is changed to unstable.
        let cond = &fs.config.load().condition;
        assert_eq!(
            Ok(&State::Unstable),
            fs.stat.on_operated_after(Operation::Read, cond)
        );

        // Change the condition.
        let mut config = (&*fs.config.load()).clone();
        config.condition = Condition::Always(State::Stable);
        fs.config.store(config);

        let cond = &fs.config.load().condition;
        assert_eq!(
            Ok(&State::Stable),
            fs.stat.on_operated_after(Operation::Read, cond)
        );
    }

    #[test]
    fn test_toggle_mode() {
        let is_unstable = true;
        let duration = &Duration::from_secs(10);
        let frequency = &Duration::from_secs(60);

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
        let duration = &Duration::from_secs(10);
        let frequency = &Duration::from_secs(60);

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
        let duration = &Duration::from_secs(10);
        let frequency = &Duration::from_secs(60);

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
        let duration = &Duration::from_secs(10);
        let frequency = &Duration::from_secs(60);

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
        let duration = &Duration::from_secs(10);
        let frequency = &Duration::from_secs(60);

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
