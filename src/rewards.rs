//! Block reward calculation and distribution (75% PoW, 25% PoS)

use crate::{PoWConfig, PoWError, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Block reward structure (100% to miners)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockReward {
    pub block_height: u64,
    pub base_reward: u128,
    pub transaction_fees: u128,
    pub total_miner_reward: u128,
}

impl BlockReward {
    pub fn new(
        block_height: u64,
        base_reward: u128,
        transaction_fees: u128,
    ) -> Result<Self> {
        let total_miner_reward = base_reward + transaction_fees;

        Ok(Self {
            block_height,
            base_reward,
            transaction_fees,
            total_miner_reward,
        })
    }

    pub fn get_miner_reward(&self) -> u128 {
        self.total_miner_reward
    }
}

/// Calculates block rewards
pub struct RewardCalculator {
    config: PoWConfig,
}

impl RewardCalculator {
    pub fn new(config: PoWConfig) -> Self {
        Self { config }
    }

    /// Calculate base block reward with halving
    pub fn calculate_base_reward(&self, block_height: u64) -> u128 {
        let halvings = block_height / self.config.halving_interval;

        // Reward halves every halving_interval blocks
        // After 64 halvings, reward becomes 0
        if halvings >= 64 {
            return 0;
        }

        self.config.base_block_reward >> halvings
    }

    /// Calculate total block reward (base + fees) - 100% to miners
    pub fn calculate_total_reward(
        &self,
        block_height: u64,
        transaction_fees: u128,
    ) -> Result<BlockReward> {
        let base_reward = self.calculate_base_reward(block_height);

        BlockReward::new(
            block_height,
            base_reward,
            transaction_fees,
        )
    }

    /// Calculate miner reward (100% of block reward + 100% of fees)
    pub fn calculate_miner_reward(
        &self,
        block_height: u64,
        transaction_fees: u128,
    ) -> Result<u128> {
        let reward = self.calculate_total_reward(block_height, transaction_fees)?;
        Ok(reward.get_miner_reward())
    }

    /// Check if halving occurs at this block
    pub fn is_halving_block(&self, block_height: u64) -> bool {
        block_height > 0 && block_height % self.config.halving_interval == 0
    }

    /// Get next halving block height
    pub fn get_next_halving_height(&self, current_height: u64) -> u64 {
        ((current_height / self.config.halving_interval) + 1) * self.config.halving_interval
    }

    /// Get blocks until next halving
    pub fn blocks_until_halving(&self, current_height: u64) -> u64 {
        self.get_next_halving_height(current_height) - current_height
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_reward_creation() {
        let reward = BlockReward::new(100, 50_000_000_000, 1_000_000);
        assert!(reward.is_ok());

        let reward = reward.unwrap();
        assert_eq!(reward.base_reward, 50_000_000_000);
        assert_eq!(reward.transaction_fees, 1_000_000);
        assert_eq!(reward.total_miner_reward, 50_001_000_000);
    }

    #[test]
    fn test_reward_calculator_creation() {
        let config = PoWConfig::default();
        let calc = RewardCalculator::new(config);
        assert_eq!(calc.config.mining_algorithm, "SHA-512");
    }

    #[test]
    fn test_calculate_base_reward() {
        let config = PoWConfig::default();
        let calc = RewardCalculator::new(config);

        let reward_at_0 = calc.calculate_base_reward(0);
        let reward_at_halving = calc.calculate_base_reward(config.halving_interval);
        let reward_at_2x_halving = calc.calculate_base_reward(config.halving_interval * 2);

        assert_eq!(reward_at_0, config.base_block_reward);
        assert_eq!(reward_at_halving, config.base_block_reward / 2);
        assert_eq!(reward_at_2x_halving, config.base_block_reward / 4);
    }

    #[test]
    fn test_calculate_total_reward() {
        let config = PoWConfig::default();
        let calc = RewardCalculator::new(config);

        let reward = calc.calculate_total_reward(0, 1_000_000).unwrap();
        assert_eq!(reward.block_height, 0);
        assert_eq!(reward.transaction_fees, 1_000_000);
    }

    #[test]
    fn test_miner_reward_100_percent() {
        let config = PoWConfig::default();
        let calc = RewardCalculator::new(config);

        let miner_reward = calc.calculate_miner_reward(0, 0).unwrap();

        // 100% to miners
        assert_eq!(miner_reward, config.base_block_reward);
    }

    #[test]
    fn test_halving_detection() {
        let config = PoWConfig::default();
        let calc = RewardCalculator::new(config);

        assert!(!calc.is_halving_block(0));
        assert!(calc.is_halving_block(config.halving_interval));
        assert!(calc.is_halving_block(config.halving_interval * 2));
    }

    #[test]
    fn test_blocks_until_halving() {
        let config = PoWConfig::default();
        let calc = RewardCalculator::new(config);

        let blocks_until = calc.blocks_until_halving(0);
        assert_eq!(blocks_until, config.halving_interval);

        let blocks_until = calc.blocks_until_halving(config.halving_interval / 2);
        assert_eq!(blocks_until, config.halving_interval / 2);
    }
}
