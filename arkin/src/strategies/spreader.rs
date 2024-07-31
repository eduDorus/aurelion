use std::sync::Arc;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::info;

use crate::{config::SpreaderConfig, state::State};

use super::Strategy;

#[derive(Clone)]
#[allow(unused)]
pub struct Spreader {
    state: Arc<State>,
    spread_in_pct: Decimal,
}

impl Spreader {
    pub fn new(state: Arc<State>, config: &SpreaderConfig) -> Self {
        Self {
            state,
            spread_in_pct: config.min_spread,
        }
    }
}

#[async_trait]
impl Strategy for Spreader {
    async fn start(&self) {
        info!("Starting spreader strategy...");
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));

        loop {
            interval.tick().await;
            info!("Spreader takes snapshot");
        }
    }
}