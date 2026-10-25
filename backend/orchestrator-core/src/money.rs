//! Money as integer USD micros (1 USD = 1_000_000), so cost sums never drift like floats.
//! It exports to TypeScript as `number`; budgets stay far below 2^53 micros.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export)]
pub struct Micros(pub i64);

impl Micros {
    pub fn saturating_add(self, other: Micros) -> Micros {
        Micros(self.0.saturating_add(other.0))
    }
}
