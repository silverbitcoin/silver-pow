//! Production-grade mining reward distribution system
//! Handles block rewards, miner payouts, and reward tracking

use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use silver_core::MIST_PER_SLVR;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Miner reward account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerRewardAccount {
    /// Miner address
    pub address: String,
    /// Total rewards earned (in satoshis)
    pub total_rewards: u128,
    /// Pending rewards (not yet paid)
    pub pending_rewards: u128,
    /// Paid rewards (already distributed)
    pub paid_rewards: u128,
    /// Number of blocks found
    pub blocks_found: u64,
    /// Last reward timestamp
    pub last_reward_time: u64,
    /// Account creation timestamp
    pub created_at: u64,
}

impl MinerRewardAccount {
    /// Create new miner reward account
    pub fn new(address: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            address,
            total_rewards: 0,
            pending_rewards: 0,
            paid_rewards: 0,
            blocks_found: 0,
            last_reward_time: now,
            created_at: now,
        }
    }

    /// Add reward to account
    pub fn add_reward(&mut self, amount: u128) {
        self.total_rewards = self.total_rewards.saturating_add(amount);
        self.pending_rewards = self.pending_rewards.saturating_add(amount);
        self.blocks_found = self.blocks_found.saturating_add(1);
        self.last_reward_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }

    /// Mark rewards as paid
    pub fn mark_paid(&mut self, amount: u128) -> Result<(), String> {
        if self.pending_rewards < amount {
            return Err(format!(
                "Insufficient pending rewards: {} < {}",
                self.pending_rewards, amount
            ));
        }

        self.pending_rewards = self.pending_rewards.saturating_sub(amount);
        self.paid_rewards = self.paid_rewards.saturating_add(amount);
        Ok(())
    }

    /// Get account balance (total - paid)
    pub fn balance(&self) -> u128 {
        self.total_rewards.saturating_sub(self.paid_rewards)
    }
}

/// Block reward record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockRewardRecord {
    /// Block height
    pub block_height: u64,
    /// Block hash
    pub block_hash: String,
    /// Miner address
    pub miner_address: String,
    /// Base block reward (in satoshis)
    pub base_reward: u128,
    /// Transaction fees collected
    pub transaction_fees: u128,
    /// Total reward (base + fees)
    pub total_reward: u128,
    /// Timestamp
    pub timestamp: u64,
}

impl BlockRewardRecord {
    /// Create new block reward record
    pub fn new(
        block_height: u64,
        block_hash: String,
        miner_address: String,
        base_reward: u128,
        transaction_fees: u128,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let total_reward = base_reward.saturating_add(transaction_fees);

        Self {
            block_height,
            block_hash,
            miner_address,
            base_reward,
            transaction_fees,
            total_reward,
            timestamp: now,
        }
    }
}

/// Reward distribution manager
pub struct RewardDistributionManager {
    /// Miner reward accounts
    accounts: Arc<RwLock<HashMap<String, MinerRewardAccount>>>,
    /// Block reward history
    reward_history: Arc<RwLock<Vec<BlockRewardRecord>>>,
    /// Total rewards distributed
    total_distributed: Arc<RwLock<u128>>,
    /// Halving interval (blocks)
    halving_interval: u64,
    /// Base block reward (in satoshis)
    base_block_reward: u128,
}

impl RewardDistributionManager {
    /// Create new reward distribution manager
    pub fn new(halving_interval: u64, base_block_reward: u128) -> Self {
        Self {
            accounts: Arc::new(RwLock::new(HashMap::new())),
            reward_history: Arc::new(RwLock::new(Vec::new())),
            total_distributed: Arc::new(RwLock::new(0)),
            halving_interval,
            base_block_reward,
        }
    }

    /// Calculate block reward with halving
    pub fn calculate_block_reward(&self, block_height: u64) -> u128 {
        let halvings = block_height / self.halving_interval;

        // Reward halves every halving_interval blocks
        // After 64 halvings, reward becomes 0
        if halvings >= 64 {
            return 0;
        }

        self.base_block_reward >> halvings
    }

    /// Record block reward and add to miner account
    pub async fn record_block_reward(
        &self,
        block_height: u64,
        block_hash: String,
        miner_address: String,
        transaction_fees: u128,
    ) -> Result<BlockRewardRecord, String> {
        // Calculate base reward with halving
        let base_reward = self.calculate_block_reward(block_height);

        // Create reward record
        let record = BlockRewardRecord::new(
            block_height,
            block_hash.clone(),
            miner_address.clone(),
            base_reward,
            transaction_fees,
        );

        // Add to miner account
        let mut accounts = self.accounts.write().await;
        let account = accounts
            .entry(miner_address.clone())
            .or_insert_with(|| MinerRewardAccount::new(miner_address.clone()));

        account.add_reward(record.total_reward);

        // Record in history
        let mut history = self.reward_history.write().await;
        history.push(record.clone());

        info!(
            "Block reward recorded: height={}, miner={}, reward={}",
            block_height, miner_address, record.total_reward
        );

        Ok(record)
    }

    /// Get miner account
    pub async fn get_miner_account(&self, address: &str) -> Option<MinerRewardAccount> {
        let accounts = self.accounts.read().await;
        accounts.get(address).cloned()
    }

    /// Get miner balance
    pub async fn get_miner_balance(&self, address: &str) -> u128 {
        let accounts = self.accounts.read().await;
        accounts.get(address).map(|acc| acc.balance()).unwrap_or(0)
    }

    /// Get all miner accounts
    pub async fn get_all_accounts(&self) -> Vec<MinerRewardAccount> {
        let accounts = self.accounts.read().await;
        accounts.values().cloned().collect()
    }

    /// Process payout to miner
    pub async fn process_payout(&self, miner_address: &str, amount: u128) -> Result<(), String> {
        let mut accounts = self.accounts.write().await;

        let account = accounts
            .get_mut(miner_address)
            .ok_or_else(|| format!("Miner account not found: {}", miner_address))?;

        // Validate sufficient balance
        if account.balance() < amount {
            return Err(format!(
                "Insufficient balance: {} < {}",
                account.balance(),
                amount
            ));
        }

        // Mark as paid
        account.mark_paid(amount)?;

        // Update total distributed
        let mut total = self.total_distributed.write().await;
        *total = total.saturating_add(amount);

        info!(
            "Payout processed: miner={}, amount={}, remaining_balance={}",
            miner_address,
            amount,
            account.balance()
        );

        Ok(())
    }

    /// Get reward history for miner
    pub async fn get_miner_reward_history(&self, miner_address: &str) -> Vec<BlockRewardRecord> {
        let history = self.reward_history.read().await;
        history
            .iter()
            .filter(|r| r.miner_address == miner_address)
            .cloned()
            .collect()
    }

    /// Get total rewards distributed
    pub async fn get_total_distributed(&self) -> u128 {
        *self.total_distributed.read().await
    }

    /// Get reward statistics
    pub async fn get_stats(&self) -> RewardStats {
        let accounts = self.accounts.read().await;
        let history = self.reward_history.read().await;
        let total_distributed = *self.total_distributed.read().await;

        let total_rewards: u128 = accounts.values().map(|a| a.total_rewards).sum();
        let total_pending: u128 = accounts.values().map(|a| a.pending_rewards).sum();
        let total_blocks: u64 = accounts.values().map(|a| a.blocks_found).sum();

        RewardStats {
            total_miners: accounts.len(),
            total_blocks_mined: total_blocks,
            total_rewards_earned: total_rewards,
            total_rewards_pending: total_pending,
            total_rewards_distributed: total_distributed,
            reward_history_size: history.len(),
        }
    }

    /// Check if halving occurs at this block
    pub fn is_halving_block(&self, block_height: u64) -> bool {
        block_height > 0 && block_height.is_multiple_of(self.halving_interval)
    }

    /// Get next halving block height
    pub fn get_next_halving_height(&self, current_height: u64) -> u64 {
        ((current_height / self.halving_interval) + 1) * self.halving_interval
    }

    /// Get blocks until next halving
    pub fn blocks_until_halving(&self, current_height: u64) -> u64 {
        self.get_next_halving_height(current_height) - current_height
    }
}

/// Reward statistics
#[derive(Debug, Clone, Serialize)]
pub struct RewardStats {
    pub total_miners: usize,
    pub total_blocks_mined: u64,
    pub total_rewards_earned: u128,
    pub total_rewards_pending: u128,
    pub total_rewards_distributed: u128,
    pub reward_history_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_miner_reward_account() {
        let mut account = MinerRewardAccount::new("miner1".to_string());
        assert_eq!(account.balance(), 0);

        let block_reward = 50 * MIST_PER_SLVR; // 50 SLVR = 5,000,000,000 MIST
        account.add_reward(block_reward as u128);
        assert_eq!(account.total_rewards, block_reward as u128);
        assert_eq!(account.pending_rewards, block_reward as u128);
        assert_eq!(account.balance(), block_reward as u128);

        account.mark_paid((block_reward / 2) as u128).unwrap();
        assert_eq!(account.pending_rewards, (block_reward / 2) as u128);
        assert_eq!(account.paid_rewards, (block_reward / 2) as u128);
        assert_eq!(account.balance(), (block_reward / 2) as u128);
    }

    #[test]
    fn test_block_reward_calculation() {
        let block_reward = 50 * MIST_PER_SLVR; // 50 SLVR = 5,000,000,000 MIST
        let manager = RewardDistributionManager::new(210000, block_reward as u128);

        // First block
        assert_eq!(manager.calculate_block_reward(0), block_reward as u128);

        // Before halving
        assert_eq!(manager.calculate_block_reward(209999), block_reward as u128);

        // After first halving
        assert_eq!(
            manager.calculate_block_reward(210000),
            (block_reward / 2) as u128
        );

        // After second halving
        assert_eq!(
            manager.calculate_block_reward(420000),
            (block_reward / 4) as u128
        );
    }

    #[test]
    fn test_halving_detection() {
        let block_reward = 50 * MIST_PER_SLVR; // 50 SLVR = 5,000,000,000 MIST
        let manager = RewardDistributionManager::new(210000, block_reward as u128);

        assert!(!manager.is_halving_block(0));
        assert!(!manager.is_halving_block(209999));
        assert!(manager.is_halving_block(210000));
        assert!(manager.is_halving_block(420000));
    }

    #[tokio::test]
    async fn test_reward_distribution() {
        let block_reward = 50 * MIST_PER_SLVR; // 50 SLVR = 5,000,000,000 MIST
        let manager = RewardDistributionManager::new(210000, block_reward as u128);

        let record = manager
            .record_block_reward(1, "hash1".to_string(), "miner1".to_string(), 100000)
            .await
            .unwrap();

        assert_eq!(record.base_reward, block_reward as u128);
        assert_eq!(record.transaction_fees, 100000);
        assert_eq!(record.total_reward, (block_reward as u128) + 100000);

        let balance = manager.get_miner_balance("miner1").await;
        assert_eq!(balance, (block_reward as u128) + 100000);
    }

    #[tokio::test]
    async fn test_payout_processing() {
        let block_reward = 50 * MIST_PER_SLVR; // 50 SLVR = 5,000,000,000 MIST
        let manager = RewardDistributionManager::new(210000, block_reward as u128);

        manager
            .record_block_reward(1, "hash1".to_string(), "miner1".to_string(), 0)
            .await
            .unwrap();

        let balance_before = manager.get_miner_balance("miner1").await;
        assert_eq!(balance_before, block_reward as u128);

        manager
            .process_payout("miner1", (block_reward / 2) as u128)
            .await
            .unwrap();

        let balance_after = manager.get_miner_balance("miner1").await;
        assert_eq!(balance_after, (block_reward / 2) as u128);
    }
}
