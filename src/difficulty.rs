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


/// REAL IMPLEMENTATION: Calculate difficulty bits from difficulty value
/// Difficulty bits format: 0xEEMMMMMM where EE is exponent (1 byte) and MMMMMM is mantissa (3 bytes)
/// Exponent must be between 3-30
/// This converts a difficulty value to the compact bits representation used in blockchain
/// 
/// Algorithm:
/// 1. Convert difficulty to target using: target = 2^256 / difficulty (for SHA-256 compatibility)
/// 2. Encode target as compact bits: find mantissa (3 bytes) and exponent (1 byte)
/// 3. Formula: bits = (exponent << 24) | mantissa
/// 4. Decode: target = mantissa * 2^(8*(exponent-3))
pub fn calculate_difficulty_bits(difficulty: u64) -> u32 {
    if difficulty == 0 {
        return 0;
    }
    
    // Maximum target for SHA-512 (all bits set)
    // For blockchain compatibility, we use the standard 0x1d00ffff
    // which represents difficulty 1
    const MAX_TARGET_BITS: u32 = 0x1d00ffff; // Bitcoin's difficulty 1 target
    
    // For difficulty 1, return the standard max target bits
    if difficulty == 1 {
        return MAX_TARGET_BITS;
    }
    
    // Decode max target bits to get reference values
    let max_exponent = (MAX_TARGET_BITS >> 24) as u8 as u32;
    let max_mantissa = MAX_TARGET_BITS & 0xFFFFFF;
    
    // Use logarithmic approach for accurate calculation
    // log2(target) = log2(max_target) - log2(difficulty)
    let log2_max_target = (max_exponent as f64 - 3.0) * 8.0 + (max_mantissa as f64).log2();
    let log2_difficulty = (difficulty as f64).log2();
    let log2_target = log2_max_target - log2_difficulty;
    
    // Calculate exponent: how many bytes we need
    // We need: log2_target = log2(mantissa) + 8*(exponent-3)
    // So: exponent = (log2_target - log2(mantissa)) / 8 + 3
    // We want mantissa to be in range [0x800000, 0xFFFFFF] (bit 23 set)
    // So: log2(mantissa) should be in range [23, 24)
    // Therefore: exponent = ceil((log2_target - 23) / 8) + 3
    
    let exponent_f64 = ((log2_target - 23.0) / 8.0).ceil() + 3.0;
    let exponent = exponent_f64.clamp(3.0, 30.0) as u32;
    
    // Calculate mantissa: the significant bits
    // mantissa = 2^(log2_target - 8*(exponent-3))
    let mantissa_bits = log2_target - 8.0 * (exponent as f64 - 3.0);
    let mantissa_f64 = 2.0_f64.powf(mantissa_bits);
    let mantissa = (mantissa_f64 as u32).clamp(1, 0xFFFFFF);
    
    // Encode as bits: (exponent << 24) | mantissa
    ((exponent & 0xFF) << 24) | (mantissa & 0xFFFFFF)
}

/// Convert difficulty bits back to difficulty value for verification
/// This is the inverse of calculate_difficulty_bits
#[allow(dead_code)]
pub fn bits_to_difficulty(bits: u32) -> u64 {
    if bits == 0 {
        return 0;
    }
    
    // Bits format: 0xEEMMMMMM where EE is exponent (1 byte) and MMMMMM is mantissa (3 bytes)
    let exponent = (bits >> 24) as u8 as u32;
    let mantissa = bits & 0xFFFFFF;
    
    // Validate exponent range
    if !(3..=30).contains(&exponent) {
        return 0;
    }
    
    // Validate mantissa
    if mantissa == 0 {
        return 0;
    }
    
    // Calculate target from bits
    // target = mantissa * 2^(8*(exponent-3))
    // In log2 space: log2(target) = log2(mantissa) + 8*(exponent-3)
    let target_bits = (mantissa as f64).log2() + 8.0 * (exponent as f64 - 3.0);
    
    // Reference max target bits (difficulty 1)
    const MAX_TARGET_BITS: u32 = 0x1d00ffff;
    let max_exponent = (MAX_TARGET_BITS >> 24) as u8 as u32;
    let max_mantissa = MAX_TARGET_BITS & 0xFFFFFF;
    let max_target_bits = (max_mantissa as f64).log2() + 8.0 * (max_exponent as f64 - 3.0);
    
    // difficulty = max_target / target
    let difficulty_f64 = 2.0_f64.powf(max_target_bits - target_bits);
    difficulty_f64 as u64
}

    #[test]
    fn test_calculate_difficulty_bits_difficulty_1() {
        // Difficulty 1 should return standard max target bits
        let bits = calculate_difficulty_bits(1);
        assert_eq!(bits, 0x1d00ffff);
    }

    #[test]
    fn test_calculate_difficulty_bits_difficulty_0() {
        // Difficulty 0 should return 0
        let bits = calculate_difficulty_bits(0);
        assert_eq!(bits, 0);
    }

    #[test]
    fn test_calculate_difficulty_bits_high_difficulty() {
        // Higher difficulty should produce higher exponent or lower mantissa
        let bits_1m = calculate_difficulty_bits(1_000_000);
        let bits_1b = calculate_difficulty_bits(1_000_000_000);
        
        // Both should be valid (exponent 3-30)
        let exp_1m = (bits_1m >> 24) as u8 as u32;
        let exp_1b = (bits_1b >> 24) as u8 as u32;
        
        assert!(exp_1m >= 3 && exp_1m <= 30, "exp_1m {} out of range", exp_1m);
        assert!(exp_1b >= 3 && exp_1b <= 30, "exp_1b {} out of range", exp_1b);
        
        // Higher difficulty should have higher exponent or lower mantissa
        assert!(bits_1b < bits_1m || exp_1b > exp_1m);
    }

    #[test]
    fn test_bits_to_difficulty_roundtrip() {
        // Test roundtrip conversion
        let original_diff = 1_000_000u64;
        let bits = calculate_difficulty_bits(original_diff);
        let recovered_diff = bits_to_difficulty(bits);
        
        // Should be close (within 5% due to floating point and rounding)
        let ratio = recovered_diff as f64 / original_diff as f64;
        assert!(ratio > 0.95 && ratio < 1.05, "Roundtrip failed: {} -> 0x{:08x} -> {} (ratio: {:.4})", original_diff, bits, recovered_diff, ratio);
    }

    #[test]
    fn test_bits_to_difficulty_invalid_exponent() {
        // Invalid exponent (too low)
        let bits_low = 0x01000000; // exponent = 1 (too low)
        let diff = bits_to_difficulty(bits_low);
        assert_eq!(diff, 0);
        
        // Invalid exponent (too high)
        let bits_high = 0xFF000000; // exponent = 255 (too high)
        let diff = bits_to_difficulty(bits_high);
        assert_eq!(diff, 0);
    }

    #[test]
    fn test_bits_format_validation() {
        // Test that bits are in correct format: 0xEEMMMMMM
        let bits = calculate_difficulty_bits(100_000);
        
        let exponent = (bits >> 24) as u8 as u32;
        let mantissa = bits & 0xFFFFFF;
        
        // Exponent must be 3-30
        assert!(exponent >= 3 && exponent <= 30, "Exponent {} out of range", exponent);
        
        // Mantissa must be 1-0xFFFFFF
        assert!(mantissa >= 1 && mantissa <= 0xFFFFFF, "Mantissa 0x{:06x} out of range", mantissa);
    }
