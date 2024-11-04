use std::fmt::{Display, Formatter};

#[derive(PartialOrd, PartialEq, Debug, Copy, Clone)]
pub struct FactorioVersion([u16; 3]);

impl From<[u16; 3]> for FactorioVersion {
    fn from(value: [u16; 3]) -> Self {
        Self(value)
    }
}

impl Display for FactorioVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}
