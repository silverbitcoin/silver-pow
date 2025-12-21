//! Production-grade difficulty adjustment system
//! Implements per-chain difficulty adjustment with persistence

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Difficulty adjustment record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DifficultyAdjustmentRecord {
    /// Block height
    pub block_height: u64,
    /// Previous difficulty
    pub previous_difficulty: u64,
    /// New difficulty
    pub new_difficulty: u64,
    /// Adjustment ratio
    pub adjustment_ratio: f64,
    /// Blocks in adjustment period
    pub blocks_in_period: u64,
    /// Time for period (milliseconds)
    pub time_for_period_ms: u64,
    /// Expected time (milliseconds)
    pub expected_time_ms: u64,
    /// Timestamp
    pub timestamp: u64,
}

impl DifficultyAdjustmentRecord {
    /// Create new difficulty adjustment record
    pub fn new(
        block_height: u64,
        previous_difficulty: u64,
        new_difficulty: u64,
        adjustment_ratio: f64,
        blocks_in_period: u64,
        time_for_period_ms: u64,
        expected_time_ms: u64,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            block_height,
            previous_difficulty,
            new_difficulty,
            adjustment_ratio,
            blocks_in_period,
            time_for_period_ms,
            expected_time_ms,
            timestamp: now,
        }
    }
}

/// Block time record for difficulty calculation
#[derive(Debug, Clone, Copy)]
pub struct BlockTimeRecord {
    /// Block height
    pub height: u64,
    /// Block timestamp (seconds)
    pub timestamp: u64,
}

/// Difficulty adjustment manager
pub struct DifficultyAdjustmentManager {
    /// Current difficulty
    current_difficulty: Arc<RwLock<u64>>,
    /// Block time history (for adjustment calculation)
    block_times: Arc<RwLock<VecDeque<BlockTimeRecord>>>,
    /// Adjustment history
    adjustment_history: Arc<RwLock<Vec<DifficultyAdjustmentRecord>>>,
    /// Adjustment interval (blocks)
    adjustment_interval: u64,
    /// Target block time (seconds)
    target_block_time_secs: u64,
    /// Minimum difficulty
    min_difficulty: u64,
    /// Maximum difficulty
    max_difficulty: u64,
    /// Maximum adjustment ratio (4x)
    max_adjustment_ratio: f64,
}

impl DifficultyAdjustmentManager {
    /// Create new difficulty adjustment manager
    pub fn new(
        initial_difficulty: u64,
        adjustment_interval: u64,
        target_block_time_secs: u64,
        min_difficulty: u64,
        max_difficulty: u64,
    ) -> Self {
        Self {
            current_difficulty: Arc::new(RwLock::new(initial_difficulty)),
            block_times: Arc::new(RwLock::new(VecDeque::new())),
            adjustment_history: Arc::new(RwLock::new(Vec::new())),
            adjustment_interval,
            target_block_time_secs,
            min_difficulty,
            max_difficulty,
            max_adjustment_ratio: 4.0,
        }
    }

    /// Record block time
    pub async fn record_block_time(&self, height: u64, timestamp: u64) {
        let mut times = self.block_times.write().await;
        times.push_back(BlockTimeRecord { height, timestamp });

        // Keep only last adjustment_interval + 1 blocks
        while times.len() > (self.adjustment_interval as usize + 1) {
            times.pop_front();
        }

        debug!("Recorded block time: height={}, timestamp={}", height, timestamp);
    }

    /// Calculate new difficulty based on block times
    pub async fn calculate_new_difficulty(&self, block_height: u64) -> Result<u64, String> {
        let times = self.block_times.read().await;

        // Need at least adjustment_interval blocks to calculate
        if times.len() < self.adjustment_interval as usize {
            debug!(
                "Not enough blocks for adjustment: {} < {}",
                times.len(),
                self.adjustment_interval
            );
            return Ok(*self.current_difficulty.read().await);
        }

        // Get first and last block times
        let first_block = times
            .front()
            .ok_or_else(|| "No block times recorded".to_string())?;
        let last_block = times
            .back()
            .ok_or_else(|| "No block times recorded".to_string())?;

        let blocks_in_period = last_block.height - first_block.height;
        let time_for_period_secs = last_block.timestamp - first_block.timestamp;
        let time_for_period_ms = time_for_period_secs * 1000;

        // Avoid division by zero
        if time_for_period_secs == 0 {
            warn!("Zero time for period, skipping adjustment");
            return Ok(*self.current_difficulty.read().await);
        }

        // Calculate expected time
        let expected_time_secs = blocks_in_period * self.target_block_time_secs;
        let expected_time_ms = expected_time_secs * 1000;

        // Calculate adjustment ratio
        let adjustment_ratio = expected_time_secs as f64 / time_for_period_secs as f64;

        // Clamp ratio to max adjustment (4x)
        let min_ratio = 1.0 / self.max_adjustment_ratio;
        let max_ratio = self.max_adjustment_ratio;
        let clamped_ratio = adjustment_ratio.max(min_ratio).min(max_ratio);

        // Calculate new difficulty
        let current_diff = *self.current_difficulty.read().await;
        let new_difficulty_f64 = current_diff as f64 * clamped_ratio;
        let new_difficulty = new_difficulty_f64 as u64;

        // Clamp to min/max
        let final_difficulty = new_difficulty
            .max(self.min_difficulty)
            .min(self.max_difficulty);

        info!(
            "Difficulty adjustment at height {}: {} -> {} (ratio: {:.4}, clamped: {:.4})",
            block_height, current_diff, final_difficulty, adjustment_ratio, clamped_ratio
        );

        // Record adjustment
        let record = DifficultyAdjustmentRecord::new(
            block_height,
            current_diff,
            final_difficulty,
            clamped_ratio,
            blocks_in_period,
            time_for_period_ms,
            expected_time_ms,
        );

        let mut history = self.adjustment_history.write().await;
        history.push(record);

        // Update current difficulty
        let mut current = self.current_difficulty.write().await;
        *current = final_difficulty;

        Ok(final_difficulty)
    }

    /// Get current difficulty
    pub async fn get_current_difficulty(&self) -> u64 {
        *self.current_difficulty.read().await
    }

    /// Set difficulty (for initialization or manual override)
    pub async fn set_difficulty(&self, difficulty: u64) {
        let mut current = self.current_difficulty.write().await;
        *current = difficulty;
        debug!("Difficulty set to {}", difficulty);
    }

    /// Check if adjustment should occur at this block
    pub fn should_adjust(&self, block_height: u64) -> bool {
        block_height > 0 && block_height.is_multiple_of(self.adjustment_interval)
    }

    /// Get next adjustment block height
    pub fn get_next_adjustment_height(&self, current_height: u64) -> u64 {
        ((current_height / self.adjustment_interval) + 1) * self.adjustment_interval
    }

    /// Get blocks until next adjustment
    pub fn blocks_until_adjustment(&self, current_height: u64) -> u64 {
        self.get_next_adjustment_height(current_height) - current_height
    }

    /// Get adjustment history
    pub async fn get_adjustment_history(&self) -> Vec<DifficultyAdjustmentRecord> {
        self.adjustment_history.read().await.clone()
    }

    /// Get recent adjustments (last N)
    pub async fn get_recent_adjustments(&self, count: usize) -> Vec<DifficultyAdjustmentRecord> {
        let history = self.adjustment_history.read().await;
        history
            .iter()
            .rev()
            .take(count)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Get difficulty statistics
    pub async fn get_stats(&self) -> DifficultyStats {
        let current = *self.current_difficulty.read().await;
        let history = self.adjustment_history.read().await;
        let times = self.block_times.read().await;

        let min_difficulty = history
            .iter()
            .map(|r| r.new_difficulty)
            .min()
            .unwrap_or(current);

        let max_difficulty = history
            .iter()
            .map(|r| r.new_difficulty)
            .max()
            .unwrap_or(current);

        let avg_adjustment_ratio = if !history.is_empty() {
            history.iter().map(|r| r.adjustment_ratio).sum::<f64>() / history.len() as f64
        } else {
            1.0
        };

        DifficultyStats {
            current_difficulty: current,
            min_difficulty,
            max_difficulty,
            avg_adjustment_ratio,
            total_adjustments: history.len(),
            blocks_recorded: times.len(),
            adjustment_interval: self.adjustment_interval,
            target_block_time_secs: self.target_block_time_secs,
        }
    }
}

/// Difficulty statistics
#[derive(Debug, Clone, Serialize)]
pub struct DifficultyStats {
    pub current_difficulty: u64,
    pub min_difficulty: u64,
    pub max_difficulty: u64,
    pub avg_adjustment_ratio: f64,
    pub total_adjustments: usize,
    pub blocks_recorded: usize,
    pub adjustment_interval: u64,
    pub target_block_time_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_difficulty_adjustment_manager_creation() {
        let manager = DifficultyAdjustmentManager::new(1000000, 2016, 600, 100000, 10000000);
        assert_eq!(manager.get_current_difficulty().await, 1000000);
    }

    #[tokio::test]
    async fn test_record_block_time() {
        let manager = DifficultyAdjustmentManager::new(1000000, 2016, 600, 100000, 10000000);

        manager.record_block_time(1, 1000).await;
        manager.record_block_time(2, 1600).await;
        manager.record_block_time(3, 2200).await;

        let stats = manager.get_stats().await;
        assert_eq!(stats.blocks_recorded, 3);
    }

    #[tokio::test]
    async fn test_should_adjust() {
        let manager = DifficultyAdjustmentManager::new(1000000, 2016, 600, 100000, 10000000);

        assert!(!manager.should_adjust(0));
        assert!(!manager.should_adjust(2015));
        assert!(manager.should_adjust(2016));
        assert!(manager.should_adjust(4032));
    }

    #[tokio::test]
    async fn test_set_difficulty() {
        let manager = DifficultyAdjustmentManager::new(1000000, 2016, 600, 100000, 10000000);

        manager.set_difficulty(2000000).await;
        assert_eq!(manager.get_current_difficulty().await, 2000000);
    }

    #[tokio::test]
    async fn test_blocks_until_adjustment() {
        let manager = DifficultyAdjustmentManager::new(1000000, 2016, 600, 100000, 10000000);

        assert_eq!(manager.blocks_until_adjustment(0), 2016);
        assert_eq!(manager.blocks_until_adjustment(1000), 1016);
        assert_eq!(manager.blocks_until_adjustment(2015), 1);
        assert_eq!(manager.blocks_until_adjustment(2016), 2016);
    }
}
