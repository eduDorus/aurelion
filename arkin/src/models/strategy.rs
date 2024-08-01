use std::fmt;
use time::OffsetDateTime;

use super::{Instrument, Weight};

#[derive(Clone)]
pub struct Signal {
    pub received_time: OffsetDateTime,
    pub event_time: OffsetDateTime,
    pub instrument: Instrument,
    pub strategy_id: String,
    pub signal: Weight,
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} {} {} {}",
            self.event_time, self.strategy_id, self.instrument, self.signal
        )
    }
}
