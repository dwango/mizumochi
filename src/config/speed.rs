use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Speed {
    Bps(usize),
    PassThrough,
}

impl FromStr for Speed {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "pass_through" {
            Ok(Speed::PassThrough)
        } else if s.ends_with("Bps") {
            let (n, _) = s.split_at(s.len() - 3);
            let mut s = n.to_string();

            let scale: usize = match s.pop().ok_or("Invalid speed")? {
                'K' => 1 << 10,
                'M' => 1 << 20,
                'G' => 1 << 30,
                r => {
                    s.push(r);
                    1
                }
            };

            let speed = s.parse::<usize>().map_err(|e| e.to_string())?;
            let speed = speed.checked_mul(scale).ok_or("overflow")?;

            Ok(Speed::Bps(speed))
        } else {
            let speed = s.parse::<usize>().map_err(|e| e.to_string())?;

            Ok(Speed::Bps(speed))
        }
    }
}

impl fmt::Display for Speed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Speed::Bps(bps) if bps < 1 << 10 => write!(f, "{}Bps", bps),
            Speed::Bps(bps) if bps < 1 << 20 => write!(f, "{}KBps", bps as f64 / (1 << 10) as f64),
            Speed::Bps(bps) if bps < 1 << 30 => write!(f, "{}MBps", bps as f64 / (1 << 20) as f64),
            Speed::Bps(bps) => write!(f, "{}GBps", bps as f64 / (1 << 30) as f64),
            Speed::PassThrough => write!(f, "PassThrough"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speed_new() {
        assert!(Speed::from_str("").is_err());
        assert!(Speed::from_str("alskjaslkdfjhasjdhfb").is_err());
        assert!(Speed::from_str("Bps").is_err());
        assert_eq!(Ok(Speed::Bps(1 << 10)), Speed::from_str("1024"));
        assert_eq!(Ok(Speed::Bps(1 << 10)), Speed::from_str("1024Bps"));
        assert_eq!(Ok(Speed::Bps(1 << 20)), Speed::from_str("1024KBps"));
        assert_eq!(Ok(Speed::Bps(1 << 30)), Speed::from_str("1024MBps"));
        assert_eq!(Ok(Speed::Bps(1 << 40)), Speed::from_str("1024GBps"));
    }
}
