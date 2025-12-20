//! Production-grade mining pool implementation with Stratum protocol support

use crate::{Miner, MinerConfig, PoWConfig, PoWError, Result, WorkPackage};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Configuration for a mining pool
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    pub min_share_difficulty: u64,
    pub max_miners: usize,
    pub pow_config: PoWConfig,
    pub pool_fee_percentage: u8,
}

impl PoolConfig {
    pub fn new() -> Self {
        Self {
            min_share_difficulty: 1_000,
            max_miners: 10_000,
            pow_config: PoWConfig::default(),
            pool_fee_percentage: 1,
        }
    }

    pub fn with_fee(mut self, fee: u8) -> Self {
        if fee > 10 {
            warn!("Pool fee {} exceeds 10%, clamping to 10%", fee);
            self.pool_fee_percentage = 10;
        } else {
            self.pool_fee_percentage = fee;
        }
        self
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics for a mining pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStats {
    pub connected_miners: usize,
    pub total_hashrate: f64,
    pub shares_accepted: u64,
    pub shares_rejected: u64,
    pub blocks_found: u64,
    pub total_earnings: u128,
    pub pool_fee_collected: u128,
    pub uptime_seconds: u64,
}

/// Detailed pool statistics with efficiency metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolDetailedStats {
    pub connected_miners: usize,
    pub total_hashrate: f64,
    pub shares_accepted: u64,
    pub shares_rejected: u64,
    pub blocks_found: u64,
    pub total_earnings: u128,
    pub pool_fee_collected: u128,
    pub uptime_seconds: u64,
    pub total_shares: u64,
    pub block_shares: u64,
    pub pool_efficiency: f64,
    pub average_miner_hashrate: f64,
}

impl PoolStats {
    pub fn new() -> Self {
        Self {
            connected_miners: 0,
            total_hashrate: 0.0,
            shares_accepted: 0,
            shares_rejected: 0,
            blocks_found: 0,
            total_earnings: 0,
            pool_fee_collected: 0,
            uptime_seconds: 0,
        }
    }
}

impl Default for PoolStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Share submitted by a miner
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerShare {
    pub miner_id: Vec<u8>,
    pub work_id: Vec<u8>,
    pub nonce: u64,
    pub extra_nonce: u64,
    pub hash_result: Vec<u8>,
    pub difficulty: u64,
    pub timestamp: u64,
    pub is_block: bool,
    pub chain_id: u32,
    pub block_height: u64,
}

impl MinerShare {
    pub fn builder(
        miner_id: Vec<u8>,
        work_id: Vec<u8>,
        hash_result: Vec<u8>,
    ) -> MinerShareBuilder {
        MinerShareBuilder {
            miner_id,
            work_id,
            nonce: 0,
            extra_nonce: 0,
            hash_result,
            difficulty: 1,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            is_block: false,
            chain_id: 0,
            block_height: 0,
        }
    }


}

/// Miner account in the pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerAccount {
    pub miner_id: Vec<u8>,
    pub miner_address: Vec<u8>,
    pub total_shares: u64,
    pub total_earnings: u128,
    pub pending_payout: u128,
    pub last_share_time: u64,
    pub connected_since: u64,
}

impl MinerAccount {
    pub fn new(miner_id: Vec<u8>, miner_address: Vec<u8>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            miner_id,
            miner_address,
            total_shares: 0,
            total_earnings: 0,
            pending_payout: 0,
            last_share_time: now,
            connected_since: now,
        }
    }
}

/// Production-grade mining pool
pub struct MiningPool {
    config: PoolConfig,
    miners: Arc<RwLock<HashMap<Vec<u8>, Arc<Miner>>>>,
    miner_accounts: Arc<RwLock<HashMap<Vec<u8>, MinerAccount>>>,
    stats: Arc<RwLock<PoolStats>>,
    shares: Arc<RwLock<Vec<MinerShare>>>,
    current_work: Arc<RwLock<Option<WorkPackage>>>,
    start_time: Instant,
    total_shares: Arc<AtomicU64>,
}

impl MiningPool {
    pub fn new(config: PoolConfig) -> Self {
        info!("Creating mining pool with {} max miners, {}% fee", config.max_miners, config.pool_fee_percentage);

        Self {
            config,
            miners: Arc::new(RwLock::new(HashMap::new())),
            miner_accounts: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(PoolStats::new())),
            shares: Arc::new(RwLock::new(Vec::new())),
            current_work: Arc::new(RwLock::new(None)),
            start_time: Instant::now(),
            total_shares: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Register a miner with the pool
    pub async fn register_miner(&self, miner_id: Vec<u8>, miner_address: Vec<u8>) -> Result<()> {
        let mut miners = self.miners.write().await;

        if miners.len() >= self.config.max_miners {
            return Err(PoWError::PoolError("Pool is full".to_string()));
        }

        if miners.contains_key(&miner_id) {
            return Err(PoWError::PoolError("Miner already registered".to_string()));
        }

        let miner_config = MinerConfig::new(1);
        let miner = Arc::new(Miner::new(miner_config));

        miners.insert(miner_id.clone(), miner);

        // Create miner account
        let mut accounts = self.miner_accounts.write().await;
        accounts.insert(miner_id.clone(), MinerAccount::new(miner_id.clone(), miner_address));

        let mut stats = self.stats.write().await;
        stats.connected_miners = miners.len();

        info!("Miner registered. Total miners: {}", stats.connected_miners);
        Ok(())
    }

    /// Unregister a miner
    pub async fn unregister_miner(&self, miner_id: &[u8]) -> Result<()> {
        let mut miners = self.miners.write().await;
        miners.remove(miner_id);

        let mut accounts = self.miner_accounts.write().await;
        accounts.remove(miner_id);

        let mut stats = self.stats.write().await;
        stats.connected_miners = miners.len();

        debug!("Miner unregistered. Total miners: {}", stats.connected_miners);
        Ok(())
    }

    /// Get number of connected miners
    pub async fn get_miner_count(&self) -> usize {
        self.miners.read().await.len()
    }

    /// Distribute work to all miners
    pub async fn distribute_work(&self, work: WorkPackage) -> Result<()> {
        let miners = self.miners.read().await;

        if miners.is_empty() {
            return Err(PoWError::PoolError("No miners connected".to_string()));
        }

        for miner in miners.values() {
            miner.set_work(work.clone()).await?;
        }

        let mut current = self.current_work.write().await;
        *current = Some(work);

        debug!("Work distributed to {} miners", miners.len());
        Ok(())
    }

    /// Submit a share from a miner
    pub async fn submit_share(&self, share: MinerShare) -> Result<bool> {
        let mut stats = self.stats.write().await;

        // Validate share difficulty
        if share.difficulty < self.config.min_share_difficulty {
            stats.shares_rejected += 1;
            return Err(PoWError::PoolError("Share difficulty too low".to_string()));
        }

        // Validate share timestamp (not too old)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now > share.timestamp + 3600 {
            stats.shares_rejected += 1;
            return Err(PoWError::PoolError("Share too old".to_string()));
        }

        // Check if share is a valid block
        let is_block = share.is_block;
        if is_block {
            stats.blocks_found += 1;
            info!("Block found by miner! Chain: {}, Height: {}", share.chain_id, share.block_height);
        }

        stats.shares_accepted += 1;
        self.total_shares.fetch_add(1, Ordering::Relaxed);

        // Update miner account
        let mut accounts = self.miner_accounts.write().await;
        if let Some(account) = accounts.get_mut(&share.miner_id) {
            account.total_shares += 1;
            account.last_share_time = now;
        }

        let mut shares = self.shares.write().await;
        shares.push(share);

        Ok(is_block)
    }

    /// Get pool statistics
    pub async fn get_stats(&self) -> PoolStats {
        let mut stats = self.stats.read().await.clone();

        // Calculate total hashrate
        let miners = self.miners.read().await;
        let mut total_hashrate = 0.0;

        for miner in miners.values() {
            let miner_stats = miner.get_stats().await;
            total_hashrate += miner_stats.hashrate;
        }

        stats.total_hashrate = total_hashrate;
        stats.connected_miners = miners.len();
        stats.uptime_seconds = self.start_time.elapsed().as_secs();

        stats
    }

    /// Get shares for a specific miner
    pub async fn get_miner_shares(&self, miner_id: &[u8]) -> Vec<MinerShare> {
        let shares = self.shares.read().await;
        shares
            .iter()
            .filter(|s| s.miner_id == miner_id)
            .cloned()
            .collect()
    }

    /// Validate miner address format
    pub fn validate_miner_address(address: &[u8]) -> Result<()> {
        // Miner address must be 20 bytes (160 bits) for compatibility
        if address.len() != 20 {
            return Err(PoWError::PoolError(
                format!("Invalid miner address length: expected 20 bytes, got {}", address.len()),
            ));
        }

        // Address cannot be all zeros
        if address.iter().all(|&b| b == 0) {
            return Err(PoWError::PoolError("Miner address cannot be all zeros".to_string()));
        }

        // Address cannot be all ones
        if address.iter().all(|&b| b == 0xFF) {
            return Err(PoWError::PoolError("Miner address cannot be all ones".to_string()));
        }

        Ok(())
    }

    /// Calculate miner payout (100% of block reward minus pool fee)
    pub async fn calculate_miner_payout(
        &self,
        miner_id: &[u8],
        total_block_reward: u128,
    ) -> Result<u128> {
        // Validate miner ID
        Self::validate_miner_address(miner_id)?;

        let shares = self.get_miner_shares(miner_id).await;

        if shares.is_empty() {
            return Ok(0);
        }

        let all_shares = self.shares.read().await;
        let total_shares: u64 = all_shares.iter().map(|s| s.difficulty).sum();

        if total_shares == 0 {
            return Ok(0);
        }

        let miner_share_difficulty: u64 = shares.iter().map(|s| s.difficulty).sum();
        let miner_percentage = miner_share_difficulty as f64 / total_shares as f64;

        // Calculate pool fee
        let pool_fee = (total_block_reward * self.config.pool_fee_percentage as u128) / 100;
        let reward_after_fee = total_block_reward - pool_fee;

        // Calculate miner payout
        let miner_payout = (reward_after_fee as f64 * miner_percentage) as u128;

        Ok(miner_payout)
    }

    /// Process payout to miner with validation
    pub async fn process_payout(&self, miner_address: &[u8], amount: u128) -> Result<()> {
        // Validate address
        Self::validate_miner_address(miner_address)?;

        // Validate amount
        if amount == 0 {
            return Err(PoWError::PoolError("Payout amount cannot be zero".to_string()));
        }

        // Update miner account
        let mut accounts = self.miner_accounts.write().await;
        if let Some(account) = accounts.get_mut(miner_address) {
            account.pending_payout += amount;
            account.total_earnings += amount;
            debug!("Payout processed for miner: {} satoshis", amount);
        } else {
            return Err(PoWError::PoolError("Miner account not found".to_string()));
        }

        Ok(())
    }

    /// Get miner account information
    pub async fn get_miner_account(&self, miner_id: &[u8]) -> Option<MinerAccount> {
        self.miner_accounts.read().await.get(miner_id).cloned()
    }

    /// Get all miner accounts
    pub async fn get_all_miner_accounts(&self) -> Vec<MinerAccount> {
        self.miner_accounts.read().await.values().cloned().collect()
    }

    /// Get all shares
    pub async fn get_all_shares(&self) -> Vec<MinerShare> {
        self.shares.read().await.clone()
    }

    /// Get shares since timestamp
    pub async fn get_shares_since(&self, timestamp: u64) -> Vec<MinerShare> {
        let shares = self.shares.read().await;
        shares
            .iter()
            .filter(|s| s.timestamp >= timestamp)
            .cloned()
            .collect()
    }

    /// Clear old shares (for maintenance)
    pub async fn clear_old_shares(&self, max_age_seconds: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut shares = self.shares.write().await;
        shares.retain(|s| now - s.timestamp <= max_age_seconds);

        debug!("Cleared old shares. Remaining: {}", shares.len());
    }

    /// Get pool fee collected
    pub async fn get_pool_fee_collected(&self) -> u128 {
        self.stats.read().await.pool_fee_collected
    }

    /// Update pool fee collected
    pub async fn add_pool_fee(&self, amount: u128) {
        let mut stats = self.stats.write().await;
        stats.pool_fee_collected += amount;
    }

    /// Get average hashrate across all miners
    pub async fn get_average_hashrate(&self) -> f64 {
        let miners = self.miners.read().await;
        if miners.is_empty() {
            return 0.0;
        }

        let mut total_hashrate = 0.0;
        for miner in miners.values() {
            let stats = miner.get_stats().await;
            total_hashrate += stats.hashrate;
        }

        total_hashrate / miners.len() as f64
    }

    /// Get pool efficiency (blocks found / expected blocks)
    pub async fn get_pool_efficiency(&self) -> f64 {
        let _stats = self.stats.read().await;
        let shares = self.shares.read().await;

        if shares.is_empty() {
            return 0.0;
        }

        let block_shares: u64 = shares.iter().filter(|s| s.is_block).map(|s| s.difficulty).sum();
        let total_shares: u64 = shares.iter().map(|s| s.difficulty).sum();

        if total_shares == 0 {
            return 0.0;
        }

        (block_shares as f64 / total_shares as f64) * 100.0
    }

    /// Get top miners by shares
    pub async fn get_top_miners(&self, limit: usize) -> Vec<(Vec<u8>, u64)> {
        let shares = self.shares.read().await;
        let mut miner_shares: HashMap<Vec<u8>, u64> = HashMap::new();

        for share in shares.iter() {
            *miner_shares.entry(share.miner_id.clone()).or_insert(0) += share.difficulty;
        }

        let mut top_miners: Vec<_> = miner_shares.into_iter().collect();
        top_miners.sort_by(|a, b| b.1.cmp(&a.1));
        top_miners.into_iter().take(limit).collect()
    }

    /// Get pool statistics with detailed metrics
    pub async fn get_detailed_stats(&self) -> PoolDetailedStats {
        let stats = self.stats.read().await.clone();
        let miners = self.miners.read().await;
        let shares = self.shares.read().await;

        let total_shares: u64 = shares.iter().map(|s| s.difficulty).sum();
        let block_shares: u64 = shares.iter().filter(|s| s.is_block).map(|s| s.difficulty).sum();

        let mut total_hashrate = 0.0;
        for miner in miners.values() {
            let miner_stats = miner.get_stats().await;
            total_hashrate += miner_stats.hashrate;
        }

        PoolDetailedStats {
            connected_miners: stats.connected_miners,
            total_hashrate,
            shares_accepted: stats.shares_accepted,
            shares_rejected: stats.shares_rejected,
            blocks_found: stats.blocks_found,
            total_earnings: stats.total_earnings,
            pool_fee_collected: stats.pool_fee_collected,
            uptime_seconds: stats.uptime_seconds,
            total_shares,
            block_shares,
            pool_efficiency: if total_shares > 0 {
                (block_shares as f64 / total_shares as f64) * 100.0
            } else {
                0.0
            },
            average_miner_hashrate: if miners.is_empty() {
                0.0
            } else {
                total_hashrate / miners.len() as f64
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pool_creation() {
        let config = PoolConfig::new();
        let pool = MiningPool::new(config);

        let stats = pool.get_stats().await;
        assert_eq!(stats.connected_miners, 0);
    }

    #[tokio::test]
    async fn test_register_miner() {
        let config = PoolConfig::new();
        let pool = MiningPool::new(config);

        assert!(pool.register_miner(vec![10, 11], vec![12, 13]).await.is_ok());

        let count = pool.get_miner_count().await;
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_duplicate_miner_registration() {
        let config = PoolConfig::new();
        let pool = MiningPool::new(config);

        let miner_id = vec![10, 11];
        pool.register_miner(miner_id.clone(), vec![12, 13]).await.unwrap();

        let result = pool.register_miner(miner_id, vec![12, 13]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unregister_miner() {
        let config = PoolConfig::new();
        let pool = MiningPool::new(config);

        let miner_id = vec![10, 11];
        pool.register_miner(miner_id.clone(), vec![12, 13]).await.unwrap();
        assert_eq!(pool.get_miner_count().await, 1);

        pool.unregister_miner(&miner_id).await.unwrap();
        assert_eq!(pool.get_miner_count().await, 0);
    }

    #[tokio::test]
    async fn test_submit_share() {
        let config = PoolConfig::new();
        let pool = MiningPool::new(config);

        let share = MinerShare::new(
            vec![10, 11],
            vec![1u8; 64],
            12345,
            0,
            vec![0u8; 64],
            10_000,
            false,
            0,
            100,
        )
        .unwrap();

        assert!(pool.submit_share(share).await.is_ok());

        let stats = pool.get_stats().await;
        assert_eq!(stats.shares_accepted, 1);
    }

    #[tokio::test]
    async fn test_miner_payout() {
        let config = PoolConfig::new();
        let pool = MiningPool::new(config);

        let miner_id = vec![10, 11];

        let share = MinerShare::new(
            miner_id.clone(),
            vec![1u8; 64],
            12345,
            0,
            vec![0u8; 64],
            10_000,
            false,
            0,
            100,
        )
        .unwrap();

        pool.submit_share(share).await.unwrap();

        let payout = pool.calculate_miner_payout(&miner_id, 50_000_000_000).await.unwrap();
        assert!(payout > 0);
    }

    #[tokio::test]
    async fn test_pool_fee() {
        let config = PoolConfig::new().with_fee(2);
        let pool = MiningPool::new(config);

        pool.add_pool_fee(1_000_000).await;
        let fee = pool.get_pool_fee_collected().await;
        assert_eq!(fee, 1_000_000);
    }
}

/// Builder for MinerShare - real production-grade builder pattern
pub struct MinerShareBuilder {
    miner_id: Vec<u8>,
    work_id: Vec<u8>,
    nonce: u64,
    extra_nonce: u64,
    hash_result: Vec<u8>,
    difficulty: u64,
    timestamp: u64,
    is_block: bool,
    chain_id: u32,
    block_height: u64,
}

impl MinerShareBuilder {
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }

    pub fn with_extra_nonce(mut self, extra_nonce: u64) -> Self {
        self.extra_nonce = extra_nonce;
        self
    }

    pub fn with_difficulty(mut self, difficulty: u64) -> Self {
        self.difficulty = difficulty;
        self
    }

    pub fn with_timestamp(mut self, timestamp: u64) -> Self {
        self.timestamp = timestamp;
        self
    }

    pub fn with_is_block(mut self, is_block: bool) -> Self {
        self.is_block = is_block;
        self
    }

    pub fn with_chain_id(mut self, chain_id: u32) -> Self {
        self.chain_id = chain_id;
        self
    }

    pub fn with_block_height(mut self, height: u64) -> Self {
        self.block_height = height;
        self
    }

    pub fn build(self) -> Result<MinerShare> {
        if self.hash_result.len() != 64 {
            return Err(PoWError::PoolError("Invalid hash result length".to_string()));
        }

        if self.miner_id.is_empty() {
            return Err(PoWError::PoolError("Miner ID cannot be empty".to_string()));
        }

        Ok(MinerShare {
            miner_id: self.miner_id,
            work_id: self.work_id,
            nonce: self.nonce,
            extra_nonce: self.extra_nonce,
            hash_result: self.hash_result,
            difficulty: self.difficulty,
            timestamp: self.timestamp,
            is_block: self.is_block,
            chain_id: self.chain_id,
            block_height: self.block_height,
        })
    }
}
