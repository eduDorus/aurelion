use tracing::info;

use crate::models::PositionUpdate;

pub mod errors;

pub trait Portfolio {
    fn handle_position_update(&self, update: &PositionUpdate);
}

pub enum PortfolioType {
    Single(SinglePortfolio),
}

impl Portfolio for PortfolioType {
    fn handle_position_update(&self, update: &PositionUpdate) {
        match self {
            PortfolioType::Single(portfolio) => portfolio.handle_position_update(update),
        }
    }
}

pub struct SinglePortfolio {}

impl SinglePortfolio {
    pub fn new() -> Self {
        SinglePortfolio {}
    }
}

impl Portfolio for SinglePortfolio {
    fn handle_position_update(&self, update: &PositionUpdate) {
        info!("Portfolio received position update: {}", update)
    }
}
