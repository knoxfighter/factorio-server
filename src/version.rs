use crate::error::ServerError;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(PartialOrd, PartialEq, Eq, Debug, Copy, Clone, Hash)]
pub struct Version([u16; 3]);

impl From<[u16; 3]> for Version {
    fn from(value: [u16; 3]) -> Self {
        Self(value)
    }
}

impl FromStr for Version {
    type Err = ServerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(ServerError::InvalidVersionFormat(format!(
                "expected 3 parts, got {}",
                parts.len()
            )));
        }

        let mut version = [0; 3];
        for (i, part) in parts.iter().enumerate() {
            version[i] = part.parse().map_err(|_| {
                ServerError::InvalidVersionFormat(format!("invalid version part: {}", part))
            })?;
        }
        Ok(Self(version))
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}
