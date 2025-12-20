//! Difficulty adjustment algorithm 

use crate::{PoWConfig, PoWError, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Difficulty adjustment information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DifficultyAdjustment {
    pub chain_id: u32,
    pub current_difficulty: u64,
    pub previous_difficulty: u64,
    pub adjustment_factor: f64,
    pub block_height: u64,
    pub timestamp: u64,
}

impl DifficultyAdjustment {
    pub fn new(
        chain_id: u32,
        current_difficulty: u64,
        previous_difficulty: u64,
        block_height: u64,
        timestamp: u64,
    ) -> Self {
        let adjustment_factor = if previous_difficulty > 0 {
            current_difficulty as f64 / previous_difficulty as f64
        } else {
            1.0
        };

        Self {
            chain_id,
            current_difficulty,
            previous_difficulty,
            adjustment_factor,
            block_height,
            timestamp,
        }
    }
}

/// Calculates difficulty adjustments
pub struct DifficultyCalculator {
    config: PoWConfig,
}

impl DifficultyCalculator {
    pub fn new(config: PoWConfig) -> Self {
        Self { config }
    }

    /// Calculate new difficulty based on block times
    pub fn calculate_difficulty(
        &self,
        current_difficulty: u64,
        actual_block_times: &[u64],
        _block_height: u64,
    ) -> Result<u64> {
        if actual_block_times.is_empty() {
            return Ok(current_difficulty);
        }

        // Calculate average block time
        let total_time: u64 = actual_block_times.iter().sum();
        let avg_block_time = total_time / actual_block_times.len() as u64;

        // Calculate adjustment ratio
        let target_time = self.config.target_block_time_ms;
        let adjustment_ratio = target_time as f64 / avg_block_time as f64;

        // Apply adjustment with limits
        let new_difficulty = (current_difficulty as f64 * adjustment_ratio) as u64;

        // Clamp to min/max
        let clamped_difficulty = new_difficulty
            .max(self.config.min_difficulty)
            .min(self.config.max_difficulty);

        debug!(
            "Difficulty adjustment: {} -> {} (ratio: {:.4})",
            current_difficulty, clamped_difficulty, adjustment_ratio
        );

        Ok(clamped_difficulty)
    }

    /// Kadena-style difficulty adjustment
    /// Adjusts difficulty per chain independently
    pub fn adjust_difficulty_kadena_style(
        &self,
        current_difficulty: u64,
        blocks_in_period: u64,
        time_for_period_ms: u64,
    ) -> Result<u64> {
        if blocks_in_period == 0 {
            return Ok(current_difficulty);
        }

        // Expected time for the period
        let expected_time_ms = blocks_in_period * self.config.target_block_time_ms;

        // Avoid division by zero
        if time_for_period_ms == 0 {
            return Ok(current_difficulty);
        }

        // Calculate adjustment
        let adjustment_ratio = expected_time_ms as f64 / time_for_period_ms as f64;

        // Apply adjustment with maximum change limit (4x max)
        let max_adjustment = 4.0;
        let min_adjustment = 0.25;
        let clamped_ratio = adjustment_ratio
            .max(min_adjustment)
            .min(max_adjustment);

        let new_difficulty = (current_difficulty as f64 * clamped_ratio) as u64;

        // Clamp to min/max
        let final_difficulty = new_difficulty
            .max(self.config.min_difficulty)
            .min(self.config.max_difficulty);

        info!(
            "Kadena-style difficulty adjustment: {} -> {} (ratio: {:.4}, clamped: {:.4})",
            current_difficulty, final_difficulty, adjustment_ratio, clamped_ratio
        );

        Ok(final_difficulty)
    }

    /// Check if difficulty adjustment is needed
    pub fn should_adjust(&self, block_height: u64) -> bool {
        block_height.is_multiple_of(self.config.difficulty_adjustment_interval)
            && block_height > 0
    }

    /// Get difficulty for a given block height (with halving)
    pub fn get_difficulty_with_halving(&self, base_difficulty: u64, block_height: u64) -> u64 {
        let halvings = block_height / self.config.halving_interval;
        
        // Difficulty increases with halvings (opposite of reward halving)
        // Each halving increases difficulty by 2x
        base_difficulty.saturating_mul(2u64.saturating_pow(halvings as u32))
    }

    /// Validate difficulty
    pub fn validate_difficulty(&self, difficulty: u64) -> Result<()> {
        if difficulty < self.config.min_difficulty {
            return Err(PoWError::InvalidDifficulty(
                format!("Difficulty {} below minimum {}", difficulty, self.config.min_difficulty),
            ));
        }

        if difficulty > self.config.max_difficulty {
            return Err(PoWError::InvalidDifficulty(
                format!("Difficulty {} exceeds maximum {}", difficulty, self.config.max_difficulty),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_difficulty_calculator_creation() {
        let config = PoWConfig::default();
        let calc = DifficultyCalculator::new(config);
        assert_eq!(calc.config.target_block_time_ms, 30_000);
    }

    #[test]
    fn test_calculate_difficulty_faster_blocks() {
        let config = PoWConfig::default();
        let calc = DifficultyCalculator::new(config);

        // Blocks came in faster than target
        let block_times = vec![15_000; 10]; // 15 seconds each instead of 30
        let new_diff = calc.calculate_difficulty(1_000_000, &block_times, 100).unwrap();

        // Difficulty should increase
        assert!(new_diff > 1_000_000);
    }

    #[test]
    fn test_calculate_difficulty_slower_blocks() {
        let config = PoWConfig::default();
        let calc = DifficultyCalculator::new(config);

        // Blocks came in slower than target
        let block_times = vec![60_000; 10]; // 60 seconds each instead of 30
        let new_diff = calc.calculate_difficulty(1_000_000, &block_times, 100).unwrap();

        // Difficulty should decrease
        assert!(new_diff < 1_000_000);
    }

    #[test]
    fn test_kadena_style_adjustment() {
        let config = PoWConfig::default();
        let calc = DifficultyCalculator::new(config);

        // 2016 blocks in 60 days (expected 42 days at 30s blocks)
        let blocks = 2016u64;
        let time_ms = 60 * 24 * 60 * 60 * 1000; // 60 days in ms

        let new_diff = calc
            .adjust_difficulty_kadena_style(1_000_000, blocks, time_ms)
            .unwrap();

        // Difficulty should decrease (blocks took longer)
        assert!(new_diff < 1_000_000);
    }

    #[test]
    fn test_should_adjust() {
        let config = PoWConfig::default();
        let calc = DifficultyCalculator::new(config);

        assert!(!calc.should_adjust(0));
        assert!(!calc.should_adjust(1000));
        assert!(calc.should_adjust(2016));
        assert!(calc.should_adjust(4032));
    }

    #[test]
    fn test_validate_difficulty() {
        let config = PoWConfig::default();
        let calc = DifficultyCalculator::new(config);

        assert!(calc.validate_difficulty(1_000_000).is_ok());
        assert!(calc.validate_difficulty(config.min_difficulty).is_ok());
        assert!(calc.validate_difficulty(config.min_difficulty - 1).is_err());
    }

    #[test]
    fn test_difficulty_with_halving() {
        let config = PoWConfig::default();
        let calc = DifficultyCalculator::new(config);

        let base_diff = 1_000_000u64;
        let diff_at_0 = calc.get_difficulty_with_halving(base_diff, 0);
        let diff_at_halving = calc.get_difficulty_with_halving(base_diff, config.halving_interval);
        let diff_at_2x_halving =
            calc.get_difficulty_with_halving(base_diff, config.halving_interval * 2);

        assert_eq!(diff_at_0, base_diff);
        assert_eq!(diff_at_halving, base_diff * 2);
        assert_eq!(diff_at_2x_halving, base_diff * 4);
    }
}
