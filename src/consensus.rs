//! Production-grade PoW consensus engine integration

use crate::{
    BlockValidator, DifficultyCalculator, Miner, MinerConfig, MiningPool, PoolConfig, PoWConfig,
    PoWError, Result, WorkPackage, WorkProof,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Chain state for PoW consensus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainState {
    pub chain_id: u32,
    pub current_height: u64,
    pub current_difficulty: u64,
    pub last_block_time: u64,
    pub total_work: u128,
    pub blocks_in_period: u64,
    pub period_start_time: u64,
}

impl ChainState {
    pub fn new(chain_id: u32) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            chain_id,
            current_height: 0,
            current_difficulty: 1_000_000,
            last_block_time: now,
            total_work: 0,
            blocks_in_period: 0,
            period_start_time: now,
        }
    }
}

/// PoW consensus engine
pub struct PoWConsensus {
    config: PoWConfig,
    chain_states: Arc<RwLock<HashMap<u32, ChainState>>>,
    validator: BlockValidator,
    difficulty_calculator: DifficultyCalculator,
    mining_pool: Arc<MiningPool>,
    miners: Arc<RwLock<HashMap<u32, Arc<Miner>>>>,
    block_history: Arc<RwLock<Vec<BlockRecord>>>,
    start_time: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockRecord {
    pub chain_id: u32,
    pub height: u64,
    pub hash: Vec<u8>,
    pub timestamp: u64,
    pub difficulty: u64,
    pub miner_address: Vec<u8>,
    pub nonce: u64,
}

impl PoWConsensus {
    pub fn new(config: PoWConfig) -> Self {
        let pool_config = PoolConfig::new();
        let mining_pool = Arc::new(MiningPool::new(pool_config));

        info!("Creating PoW consensus engine with SHA-512 mining");

        Self {
            config,
            chain_states: Arc::new(RwLock::new(HashMap::new())),
            validator: BlockValidator::new(config),
            difficulty_calculator: DifficultyCalculator::new(config),
            mining_pool,
            miners: Arc::new(RwLock::new(HashMap::new())),
            block_history: Arc::new(RwLock::new(Vec::new())),
            start_time: Instant::now(),
        }
    }

    /// Initialize a new chain
    pub async fn initialize_chain(&self, chain_id: u32) -> Result<()> {
        if chain_id >= 20 {
            return Err(PoWError::InvalidBlock("Invalid chain ID".to_string()));
        }

        let mut states = self.chain_states.write().await;
        if states.contains_key(&chain_id) {
            return Err(PoWError::InvalidBlock("Chain already initialized".to_string()));
        }

        states.insert(chain_id, ChainState::new(chain_id));

        // Create miner for this chain
        let miner_config = MinerConfig::new(4);
        let miner = Arc::new(Miner::new(miner_config));

        let mut miners = self.miners.write().await;
        miners.insert(chain_id, miner);

        info!("Chain {} initialized", chain_id);
        Ok(())
    }

    /// Get current chain state
    pub async fn get_chain_state(&self, chain_id: u32) -> Result<ChainState> {
        let states = self.chain_states.read().await;
        states
            .get(&chain_id)
            .cloned()
            .ok_or_else(|| PoWError::InvalidBlock("Chain not found".to_string()))
    }

    /// Create work package for mining
    pub async fn create_work_package(
        &self,
        chain_id: u32,
        block_hash: Vec<u8>,
        parent_hash: Vec<u8>,
        merkle_root: Vec<u8>,
    ) -> Result<WorkPackage> {
        let state = self.get_chain_state(chain_id).await?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let work = WorkPackage::new(
            chain_id,
            state.current_height + 1,
            block_hash,
            parent_hash,
            merkle_root,
            now,
            state.current_difficulty,
        )?;

        debug!(
            "Created work package for chain {} height {} difficulty {}",
            chain_id, state.current_height + 1, state.current_difficulty
        );

        Ok(work)
    }

    /// Submit a block (proof of work)
    pub async fn submit_block(&self, proof: WorkProof) -> Result<bool> {
        let mut states = self.chain_states.write().await;
        let state = states
            .get_mut(&proof.chain_id)
            .ok_or_else(|| PoWError::InvalidBlock("Chain not found".to_string()))?;

        // Validate block height
        if proof.block_height != state.current_height + 1 {
            return Err(PoWError::InvalidBlock(
                format!(
                    "Invalid block height: expected {}, got {}",
                    state.current_height + 1,
                    proof.block_height
                ),
            ));
        }

        // Validate proof of work
        let target = WorkPackage::calculate_target_from_difficulty(state.current_difficulty)?;
        if proof.hash_result.as_slice() > target.as_slice() {
            return Err(PoWError::WorkProofVerificationFailed);
        }

        // Update chain state
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let block_time = now - state.last_block_time;
        state.current_height += 1;
        state.last_block_time = now;
        state.blocks_in_period += 1;
        state.total_work += proof.difficulty_achieved as u128;

        // Check if difficulty adjustment is needed
        let period_elapsed = now - state.period_start_time;
        if state.blocks_in_period >= self.config.difficulty_adjustment_interval {
            let new_difficulty = self
                .difficulty_calculator
                .adjust_difficulty_kadena_style(
                    state.current_difficulty,
                    state.blocks_in_period,
                    period_elapsed * 1000,
                )?;

            info!(
                "Difficulty adjustment for chain {}: {} -> {} (period: {} blocks, {} ms)",
                proof.chain_id, state.current_difficulty, new_difficulty, state.blocks_in_period, period_elapsed * 1000
            );

            state.current_difficulty = new_difficulty;
            state.blocks_in_period = 0;
            state.period_start_time = now;
        }

        // Record block
        let mut history = self.block_history.write().await;
        history.push(BlockRecord {
            chain_id: proof.chain_id,
            height: proof.block_height,
            hash: proof.hash_result.clone(),
            timestamp: now,
            difficulty: state.current_difficulty,
            miner_address: proof.miner_address.clone(),
            nonce: proof.nonce,
        });

        info!(
            "Block accepted for chain {} at height {} (time: {}s, difficulty: {})",
            proof.chain_id, proof.block_height, block_time, state.current_difficulty
        );

        Ok(true)
    }

    /// Get block history for a chain
    pub async fn get_block_history(&self, chain_id: u32, limit: usize) -> Vec<BlockRecord> {
        let history = self.block_history.read().await;
        history
            .iter()
            .filter(|b| b.chain_id == chain_id)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Get mining pool
    pub fn get_mining_pool(&self) -> Arc<MiningPool> {
        Arc::clone(&self.mining_pool)
    }

    /// Get consensus statistics
    pub async fn get_stats(&self) -> ConsensusStats {
        let states = self.chain_states.read().await;
        let history = self.block_history.read().await;

        let total_blocks: u64 = history.len() as u64;
        let total_work: u128 = history.iter().map(|b| b.difficulty as u128).sum();

        let avg_difficulty = if !history.is_empty() {
            total_work / total_blocks as u128
        } else {
            0
        };

        ConsensusStats {
            active_chains: states.len(),
            total_blocks,
            total_work,
            average_difficulty: avg_difficulty,
            uptime_seconds: self.start_time.elapsed().as_secs(),
        }
    }

    /// Get miner for a chain
    pub async fn get_miner(&self, chain_id: u32) -> Option<Arc<Miner>> {
        self.miners.read().await.get(&chain_id).cloned()
    }

    /// Validate a block header using the block validator
    pub fn validate_block_header(&self, header: &crate::BlockHeader) -> Result<()> {
        self.validator.validate_header(header)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusStats {
    pub active_chains: usize,
    pub total_blocks: u64,
    pub total_work: u128,
    pub average_difficulty: u128,
    pub uptime_seconds: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_consensus_creation() {
        let config = PoWConfig::default();
        let consensus = PoWConsensus::new(config);

        let stats = consensus.get_stats().await;
        assert_eq!(stats.active_chains, 0);
    }

    #[tokio::test]
    async fn test_initialize_chain() {
        let config = PoWConfig::default();
        let consensus = PoWConsensus::new(config);

        assert!(consensus.initialize_chain(0).await.is_ok());

        let state = consensus.get_chain_state(0).await;
        assert!(state.is_ok());
        assert_eq!(state.unwrap().chain_id, 0);
    }

    #[tokio::test]
    async fn test_duplicate_chain_initialization() {
        let config = PoWConfig::default();
        let consensus = PoWConsensus::new(config);

        consensus.initialize_chain(0).await.unwrap();
        let result = consensus.initialize_chain(0).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_work_package() {
        let config = PoWConfig::default();
        let consensus = PoWConsensus::new(config);

        consensus.initialize_chain(0).await.unwrap();

        let work = consensus
            .create_work_package(0, vec![1u8; 32], vec![2u8; 32], vec![3u8; 32])
            .await;

        assert!(work.is_ok());
        let work = work.unwrap();
        assert_eq!(work.chain_id, 0);
        assert_eq!(work.block_height, 1);
    }

    #[tokio::test]
    async fn test_get_stats() {
        let config = PoWConfig::default();
        let consensus = PoWConsensus::new(config);

        consensus.initialize_chain(0).await.unwrap();

        let stats = consensus.get_stats().await;
        assert_eq!(stats.active_chains, 1);
        assert_eq!(stats.total_blocks, 0);
    }
}
