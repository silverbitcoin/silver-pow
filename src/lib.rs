//! Pure Proof-of-Work (PoW) Mining Engine for SilverBitcoin
//!
//! This module implements Bitcoin-style pure PoW consensus where:
//! - Miners solve SHA-512 hash puzzles
//! - Difficulty adjusts per chain (Kadena-style)
//! - Block rewards: 100% to miners (no PoS)
//! - Mining pools supported via Stratum protocol
//! - Quantum-resistant signatures for transactions

pub mod block_builder;
pub mod block_submission;
pub mod block_validator;
pub mod consensus;
pub mod difficulty;
pub mod difficulty_adjustment;
pub mod miner;
pub mod mining_pool;
pub mod reward_distribution;
pub mod rewards;
pub mod stratum;
pub mod stratum_client;
pub mod stratum_pool;
pub mod transaction_engine;
pub mod websocket_server;
pub mod work;

pub use block_builder::{Block, BlockBuilder, BlockHeader as BH, CoinbaseTransaction};
pub use block_submission::{BlockSubmissionHandler, BlockSubmissionResult, SubmissionStatus};
pub use block_validator::{BlockHeader, BlockValidator};
pub use consensus::{BlockRecord, ChainState, ConsensusStats, PoWConsensus};
pub use difficulty::{
    bits_to_difficulty, calculate_difficulty_bits, DifficultyAdjustment, DifficultyCalculator,
};
pub use difficulty_adjustment::{
    DifficultyAdjustmentManager, DifficultyAdjustmentRecord, DifficultyStats,
};
pub use miner::{Miner, MinerConfig, MinerStats};
pub use mining_pool::{
    MinerAccount, MinerShare, MiningPool, PoolConfig, PoolDetailedStats, PoolStats,
};
pub use reward_distribution::{
    BlockRewardRecord, MinerRewardAccount, RewardDistributionManager, RewardStats,
};
pub use rewards::{BlockReward, RewardCalculator};
pub use stratum::{StratumClient, StratumMessage, StratumServer};
pub use stratum_client::StratumClient as StratumPoolClient;
pub use transaction_engine::{
    Transaction, TransactionEngine, TransactionEngineStats, TransactionStatus,
};
pub use websocket_server::{BlockchainEvent, SubscriptionType, WebSocketServer};
pub use work::{WorkPackage, WorkProof};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum PoWError {
    #[error("Invalid difficulty: {0}")]
    InvalidDifficulty(String),

    #[error("Invalid work proof")]
    InvalidWorkProof,

    #[error("Work proof verification failed")]
    WorkProofVerificationFailed,

    #[error("Difficulty adjustment failed: {0}")]
    DifficultyAdjustmentFailed(String),

    #[error("Mining pool error: {0}")]
    PoolError(String),

    #[error("Reward calculation error: {0}")]
    RewardCalculationError(String),

    #[error("Invalid block: {0}")]
    InvalidBlock(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, PoWError>;

/// Configuration for the PoW system
#[derive(Debug, Clone, Copy)]
pub struct PoWConfig {
    /// Target block time in milliseconds
    pub target_block_time_ms: u64,
    /// Difficulty adjustment interval (in blocks)
    pub difficulty_adjustment_interval: u64,
    /// Initial difficulty
    pub initial_difficulty: u64,
    /// Minimum difficulty
    pub min_difficulty: u64,
    /// Maximum difficulty
    pub max_difficulty: u64,
    /// Base block reward in satoshis (100% to miners)
    pub base_block_reward: u128,
    /// Halving interval (in blocks)
    pub halving_interval: u64,
    /// Mining algorithm: SHA-512
    pub mining_algorithm: &'static str,
}

impl Default for PoWConfig {
    fn default() -> Self {
        Self {
            target_block_time_ms: 30_000,         // 30 seconds per chain
            difficulty_adjustment_interval: 2016, // ~2 weeks at 30s blocks
            initial_difficulty: 1_000_000,
            min_difficulty: 1_000,
            max_difficulty: u64::MAX,
            base_block_reward: 50_000_000_000, // 500 SILVER in satoshis
            halving_interval: 210_000,         // Similar to Bitcoin
            mining_algorithm: "SHA-512",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PoWConfig::default();
        assert_eq!(config.mining_algorithm, "SHA-512");
        assert_eq!(config.target_block_time_ms, 30_000);
        assert_eq!(config.halving_interval, 210_000);
    }

    #[test]
    fn test_config_values() {
        let config = PoWConfig::default();
        assert!(config.base_block_reward > 0);
        assert!(config.initial_difficulty > 0);
    }
}
