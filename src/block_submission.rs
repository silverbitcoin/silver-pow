//! Production-grade block submission handler
//! Submits mined blocks to the blockchain node with full validation

use crate::block_builder::{Block, BlockBuilder};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Block submission result
#[derive(Debug, Clone)]
pub struct BlockSubmissionResult {
    /// Block hash
    pub block_hash: String,
    /// Block height
    pub block_height: u64,
    /// Miner address
    pub miner_address: String,
    /// Submission status
    pub status: SubmissionStatus,
    /// Error message if failed
    pub error: Option<String>,
    /// Timestamp
    pub timestamp: u64,
}

/// Submission status
#[derive(Debug, Clone, PartialEq)]
pub enum SubmissionStatus {
    /// Block accepted by node
    Accepted,
    /// Block rejected by node
    Rejected,
    /// Submission failed (network error)
    Failed,
}

/// Block submission handler
pub struct BlockSubmissionHandler {
    /// Node RPC URL
    node_rpc_url: String,
    /// HTTP client
    http_client: reqwest::Client,
    /// Previous block hash (for validation)
    prev_block_hash: Arc<tokio::sync::RwLock<[u8; 32]>>,
    /// Current block height
    current_height: Arc<tokio::sync::RwLock<u64>>,
}

impl BlockSubmissionHandler {
    /// Create new block submission handler
    pub fn new(node_rpc_url: String) -> Self {
        Self {
            node_rpc_url,
            http_client: reqwest::Client::new(),
            prev_block_hash: Arc::new(tokio::sync::RwLock::new([0u8; 32])),
            current_height: Arc::new(tokio::sync::RwLock::new(0)),
        }
    }

    /// Update previous block hash (called when new block is confirmed)
    pub async fn update_prev_block_hash(&self, hash: [u8; 32], height: u64) {
        let mut prev = self.prev_block_hash.write().await;
        *prev = hash;

        let mut h = self.current_height.write().await;
        *h = height;

        debug!("Updated prev block hash for height {}", height);
    }

    /// Submit block to node with full validation
    pub async fn submit_block(
        &self,
        nonce: u32,
        block_height: u64,
        miner_address: String,
        reward_amount: u128,
        transaction_fees: u128,
        difficulty_bits: u32,
    ) -> Result<BlockSubmissionResult, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("Time error: {}", e))?
            .as_secs();

        // Get previous block hash
        let prev_hash = *self.prev_block_hash.read().await;

        // Build block
        let builder = BlockBuilder::new(1, prev_hash, [0u8; 32], difficulty_bits);
        let block = builder
            .build(
                nonce,
                block_height,
                miner_address.clone(),
                reward_amount,
                transaction_fees,
            )
            .map_err(|e| format!("Block build failed: {}", e))?;

        let block_hash = block.hash_hex();

        // Validate block before submission
        if let Err(e) = self.validate_block(&block).await {
            warn!("Block validation failed: {}", e);
            return Ok(BlockSubmissionResult {
                block_hash: block_hash.clone(),
                block_height,
                miner_address,
                status: SubmissionStatus::Rejected,
                error: Some(e),
                timestamp: now,
            });
        }

        // Submit to node
        match self.submit_to_node(&block).await {
            Ok(accepted) => {
                if accepted {
                    info!(
                        "Block #{} submitted successfully: {} by {}",
                        block_height, block_hash, miner_address
                    );

                    // Update prev block hash for next block (use first 32 bytes)
                    let hash_64 = block.hash();
                    let mut hash_32 = [0u8; 32];
                    hash_32.copy_from_slice(&hash_64[0..32]);
                    self.update_prev_block_hash(hash_32, block_height).await;

                    Ok(BlockSubmissionResult {
                        block_hash,
                        block_height,
                        miner_address,
                        status: SubmissionStatus::Accepted,
                        error: None,
                        timestamp: now,
                    })
                } else {
                    warn!("Block #{} rejected by node", block_height);
                    Ok(BlockSubmissionResult {
                        block_hash,
                        block_height,
                        miner_address,
                        status: SubmissionStatus::Rejected,
                        error: Some("Node rejected block".to_string()),
                        timestamp: now,
                    })
                }
            }
            Err(e) => {
                error!("Block submission failed: {}", e);
                Ok(BlockSubmissionResult {
                    block_hash,
                    block_height,
                    miner_address,
                    status: SubmissionStatus::Failed,
                    error: Some(e),
                    timestamp: now,
                })
            }
        }
    }

    /// Validate block before submission
    async fn validate_block(&self, block: &Block) -> Result<(), String> {
        // Validate block header
        if block.header.version != 1 {
            return Err(format!("Invalid block version: {}", block.header.version));
        }

        // Validate timestamp (not too far in future)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("Time error: {}", e))?
            .as_secs();

        if block.header.timestamp as u64 > now + 7200 {
            return Err("Block timestamp too far in future".to_string());
        }

        // Validate coinbase
        if block.coinbase.miner_address.is_empty() {
            return Err("Coinbase missing miner address".to_string());
        }

        if block.coinbase.reward_amount == 0 {
            return Err("Coinbase reward is zero".to_string());
        }

        // Validate block height
        let current_height = *self.current_height.read().await;
        if block.height != current_height + 1 {
            return Err(format!(
                "Invalid block height: expected {}, got {}",
                current_height + 1,
                block.height
            ));
        }

        debug!("Block validation passed for height {}", block.height);
        Ok(())
    }

    /// Submit block to node via RPC
    async fn submit_to_node(&self, block: &Block) -> Result<bool, String> {
        let block_hex = block.to_hex();

        let request = json!({
            "jsonrpc": "2.0",
            "method": "submitblock",
            "params": [block_hex],
            "id": 1
        });

        debug!("Submitting block to node: {}", self.node_rpc_url);

        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.http_client
                .post(&self.node_rpc_url)
                .json(&request)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => {
                match response.json::<Value>().await {
                    Ok(result) => {
                        if result["error"].is_null() {
                            debug!("Node accepted block");
                            Ok(true)
                        } else {
                            let error_msg = result["error"]["message"]
                                .as_str()
                                .unwrap_or("Unknown error");
                            warn!("Node rejected block: {}", error_msg);
                            Ok(false)
                        }
                    }
                    Err(e) => Err(format!("Failed to parse node response: {}", e)),
                }
            }
            Ok(Err(e)) => Err(format!("RPC request failed: {}", e)),
            Err(_) => Err("RPC request timeout".to_string()),
        }
    }

    /// Get block submission stats
    pub async fn get_stats(&self) -> BlockSubmissionStats {
        let height = *self.current_height.read().await;
        BlockSubmissionStats {
            current_height: height,
            prev_block_hash: hex::encode(*self.prev_block_hash.read().await),
        }
    }
}

/// Block submission statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct BlockSubmissionStats {
    pub current_height: u64,
    pub prev_block_hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_block_submission_handler_creation() {
        let handler = BlockSubmissionHandler::new("http://localhost:8332".to_string());
        let stats = handler.get_stats().await;
        assert_eq!(stats.current_height, 0);
    }

    #[tokio::test]
    async fn test_update_prev_block_hash() {
        let handler = BlockSubmissionHandler::new("http://localhost:8332".to_string());
        let hash = [1u8; 32];
        handler.update_prev_block_hash(hash, 1).await;

        let stats = handler.get_stats().await;
        assert_eq!(stats.current_height, 1);
        assert_eq!(stats.prev_block_hash, hex::encode(hash));
    }
}
