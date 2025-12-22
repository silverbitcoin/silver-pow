//! Production-grade individual miner implementation with real SHA-512 mining

use crate::{PoWConfig, Result, WorkPackage, WorkProof};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Configuration for a miner
#[derive(Debug, Clone, Copy)]
pub struct MinerConfig {
    pub num_threads: usize,
    pub pow_config: PoWConfig,
}

impl MinerConfig {
    pub fn new(num_threads: usize) -> Self {
        Self {
            num_threads,
            pow_config: PoWConfig::default(),
        }
    }

    pub fn with_config(num_threads: usize, pow_config: PoWConfig) -> Self {
        Self {
            num_threads,
            pow_config,
        }
    }
}

/// Statistics for a miner
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerStats {
    pub total_hashes: u64,
    pub valid_proofs: u64,
    pub invalid_proofs: u64,
    pub current_difficulty: u64,
    pub hashrate: f64,
    pub uptime_seconds: u64,
    pub last_proof_time: u64,
    pub average_time_per_proof: f64,
}

impl MinerStats {
    pub fn new() -> Self {
        Self {
            total_hashes: 0,
            valid_proofs: 0,
            invalid_proofs: 0,
            current_difficulty: 1_000_000,
            hashrate: 0.0,
            uptime_seconds: 0,
            last_proof_time: 0,
            average_time_per_proof: 0.0,
        }
    }
}

impl Default for MinerStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Individual miner with real SHA-512 mining
pub struct Miner {
    #[allow(dead_code)]
    config: MinerConfig,
    stats: Arc<RwLock<MinerStats>>,
    current_work: Arc<RwLock<Option<WorkPackage>>>,
    total_hashes: Arc<AtomicU64>,
    start_time: Instant,
    proof_times: Arc<RwLock<Vec<u64>>>,
}

impl Miner {
    pub fn new(config: MinerConfig) -> Self {
        info!(
            "Creating miner with {} threads, SHA-512 mining",
            config.num_threads
        );

        Self {
            config,
            stats: Arc::new(RwLock::new(MinerStats::new())),
            current_work: Arc::new(RwLock::new(None)),
            total_hashes: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            proof_times: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn set_work(&self, work: WorkPackage) -> Result<()> {
        let mut current = self.current_work.write().await;
        *current = Some(work);
        debug!("Miner received new work package");
        Ok(())
    }

    pub async fn get_current_work(&self) -> Option<WorkPackage> {
        self.current_work.read().await.clone()
    }

    /// Mine a single work package using SHA-512
    /// This is the real mining loop 
    pub async fn mine_work(&self, work: WorkPackage) -> Result<Option<WorkProof>> {
        self.mine_work_with_address(work, vec![0u8; 20]).await
    }

    /// Mine with a specific miner address
    pub async fn mine_work_with_address(&self, work: WorkPackage, miner_address: Vec<u8>) -> Result<Option<WorkProof>> {
        let target = work.target.clone();
        let header = work.get_header_for_hashing();
        let work_id = work.work_id.clone();
        let chain_id = work.chain_id;
        let block_height = work.block_height;
        let block_hash = work.block_hash.clone();

        let start_time = Instant::now();
        let mut nonce = 0u64;
        let mut extra_nonce = 0u64;

        // Real mining loop - try different nonces until we find a valid proof
        loop {
            // Create hash input with nonce
            let mut hash_input = header.clone();
            hash_input.extend_from_slice(&nonce.to_le_bytes());
            hash_input.extend_from_slice(&extra_nonce.to_le_bytes());

            // Calculate SHA-512 hash
            let hash_result = WorkPackage::calculate_sha512_hash(&hash_input);

            // Increment hash counter
            self.total_hashes.fetch_add(1, Ordering::Relaxed);

            // Check if hash meets target
            if hash_result.as_slice() <= target.as_slice() {
                let proof_time = start_time.elapsed().as_secs();

                let proof = WorkProof::builder(
                    work_id.clone(),
                    chain_id,
                    block_hash.clone(),
                    hash_result,
                    miner_address,
                )
                .with_block_height(block_height)
                .with_nonce(nonce)
                .with_extra_nonce(extra_nonce)
                .with_timestamp(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                )
                .build()?;

                // Update stats
                let mut stats = self.stats.write().await;
                stats.valid_proofs += 1;
                stats.total_hashes = self.total_hashes.load(Ordering::Relaxed);
                stats.last_proof_time = proof_time;

                // Update average time per proof
                let mut times = self.proof_times.write().await;
                times.push(proof_time);
                if times.len() > 100 {
                    times.remove(0);
                }
                stats.average_time_per_proof =
                    times.iter().sum::<u64>() as f64 / times.len() as f64;

                info!(
                    "Miner found valid SHA-512 proof at nonce {} for chain {} height {} in {} seconds",
                    nonce, chain_id, block_height, proof_time
                );

                return Ok(Some(proof));
            }

            // Increment nonce
            nonce = nonce.wrapping_add(1);

            // If nonce overflows, increment extra_nonce
            if nonce == 0 {
                extra_nonce = extra_nonce.wrapping_add(1);
                debug!(
                    "Nonce overflow, incrementing extra_nonce to {}",
                    extra_nonce
                );
            }

            // Periodically check for new work (every 1M hashes)
            if nonce.is_multiple_of(1_000_000) {
                if let Some(new_work) = self.get_current_work().await {
                    if new_work.work_id != work.work_id {
                        debug!("Switching to new work package after {} hashes", nonce);
                        return Ok(None);
                    }
                }

                // Update stats periodically
                let mut stats = self.stats.write().await;
                stats.total_hashes = self.total_hashes.load(Ordering::Relaxed);
                stats.uptime_seconds = self.start_time.elapsed().as_secs();
                if stats.uptime_seconds > 0 {
                    stats.hashrate = stats.total_hashes as f64 / stats.uptime_seconds as f64;
                }
            }
        }
    }

    /// Mine continuously with work updates
    pub async fn mine_continuous(&self) -> Result<()> {
        loop {
            if let Some(work) = self.get_current_work().await {
                match self.mine_work(work).await {
                    Ok(Some(proof)) => {
                        debug!("Found proof: chain={}, height={}", proof.chain_id, proof.block_height);
                    }
                    Ok(None) => {
                        // Work changed, continue to next iteration
                        continue;
                    }
                    Err(e) => {
                        error!("Mining error: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                }
            } else {
                // No work available, wait
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }

    pub async fn get_stats(&self) -> MinerStats {
        let mut stats = self.stats.read().await.clone();
        stats.total_hashes = self.total_hashes.load(Ordering::Relaxed);
        stats.uptime_seconds = self.start_time.elapsed().as_secs();

        if stats.uptime_seconds > 0 {
            stats.hashrate = stats.total_hashes as f64 / stats.uptime_seconds as f64;
        }

        stats
    }

    pub fn get_total_hashes(&self) -> u64 {
        self.total_hashes.load(Ordering::Relaxed)
    }

    pub fn get_uptime(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    pub async fn reset_stats(&self) {
        let mut stats = self.stats.write().await;
        *stats = MinerStats::new();
        self.total_hashes.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_miner_config_creation() {
        let config = MinerConfig::new(4);
        assert_eq!(config.num_threads, 4);
    }

    #[tokio::test]
    async fn test_miner_creation() {
        let config = MinerConfig::new(4);
        let miner = Miner::new(config);
        let stats = miner.get_stats().await;

        assert_eq!(stats.total_hashes, 0);
        assert_eq!(stats.valid_proofs, 0);
    }

    #[tokio::test]
    async fn test_set_work() {
        let config = MinerConfig::new(4);
        let miner = Miner::new(config);

        let work = WorkPackage::new(
            0,
            100,
            vec![1u8; 32],
            vec![2u8; 32],
            vec![3u8; 32],
            1000,
            1_000_000,
        )
        .unwrap();

        assert!(miner.set_work(work).await.is_ok());

        let current = miner.get_current_work().await;
        assert!(current.is_some());
    }

    #[tokio::test]
    async fn test_miner_stats() {
        let config = MinerConfig::new(4);
        let miner = Miner::new(config);
        let stats = miner.get_stats().await;

        assert_eq!(stats.total_hashes, 0);
        assert_eq!(stats.valid_proofs, 0);
    }

    #[tokio::test]
    async fn test_reset_stats() {
        let config = MinerConfig::new(4);
        let miner = Miner::new(config);

        miner.total_hashes.store(1000, Ordering::Relaxed);
        let stats = miner.get_stats().await;
        assert_eq!(stats.total_hashes, 1000);

        miner.reset_stats().await;
        let stats = miner.get_stats().await;
        assert_eq!(stats.total_hashes, 0);
    }
}
