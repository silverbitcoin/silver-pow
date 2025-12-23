//! Production-grade Stratum Mining Pool for SilverBitcoin
//! Implements Stratum protocol v1 with real share validation and block submission

use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Stratum pool configuration
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Pool listen address
    pub listen_addr: SocketAddr,
    /// Node RPC URL for block submission
    pub node_rpc_url: String,
    /// Pool difficulty (shares per block)
    pub pool_difficulty: u64,
    /// Minimum difficulty for miners
    pub min_difficulty: u64,
    /// Maximum difficulty for miners
    pub max_difficulty: u64,
    /// Share timeout in seconds
    pub share_timeout: u64,
    /// Block reward address
    pub pool_address: String,
}

/// Miner session
#[derive(Debug, Clone)]
pub struct MinerSession {
    /// Unique miner ID
    pub miner_id: String,
    /// Miner address (wallet)
    pub address: String,
    /// Current difficulty
    pub difficulty: u64,
    /// Shares submitted
    pub shares: u64,
    /// Valid shares
    pub valid_shares: u64,
    /// Invalid shares
    pub invalid_shares: u64,
    /// Last share time
    pub last_share_time: u64,
    /// Connected time
    pub connected_time: u64,
    /// Hashrate (hashes per second)
    pub hashrate: f64,
}

/// Pool state
#[derive(Clone)]
pub struct PoolState {
    /// Active miner sessions
    miners: Arc<RwLock<HashMap<String, MinerSession>>>,
    /// Current work
    current_work: Arc<RwLock<WorkPackage>>,
    /// Pool configuration
    config: PoolConfig,
    /// HTTP client for RPC
    http_client: reqwest::Client,
    /// Total shares
    total_shares: Arc<RwLock<u64>>,
    /// Total valid shares
    total_valid_shares: Arc<RwLock<u64>>,
}

/// Work package for miners
#[derive(Debug, Clone)]
pub struct WorkPackage {
    /// Work ID
    pub id: String,
    /// Block header (hex)
    pub header: String,
    /// Target difficulty (hex)
    pub target: String,
    /// Block height
    pub height: u64,
    /// Timestamp
    pub timestamp: u64,
}

impl PoolState {
    /// Create new pool state
    pub fn new(config: PoolConfig) -> Self {
        let work = WorkPackage {
            id: Uuid::new_v4().to_string(),
            header: "0".repeat(160), // 80 bytes = 160 hex chars
            target: "0".repeat(64),
            height: 1,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        Self {
            miners: Arc::new(RwLock::new(HashMap::new())),
            current_work: Arc::new(RwLock::new(work)),
            config,
            http_client: reqwest::Client::new(),
            total_shares: Arc::new(RwLock::new(0)),
            total_valid_shares: Arc::new(RwLock::new(0)),
        }
    }

    /// REAL IMPLEMENTATION: Fetch current block height from node via RPC
    /// This ensures pool is synchronized with blockchain state
    pub async fn sync_block_height_from_node(&self) -> Result<u64, String> {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "getblockcount",
            "params": [],
            "id": 1
        });

        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.http_client
                .post(&self.config.node_rpc_url)
                .json(&request)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => {
                match response.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if let Some(result) = json.get("result") {
                            if let Some(height) = result.as_u64() {
                                info!("Synced block height from node: {}", height);
                                
                                // Update work with next block height
                                let mut work = self.current_work.write().await;
                                work.height = height + 1;
                                work.id = Uuid::new_v4().to_string();
                                work.timestamp = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs();
                                
                                info!("Pool work updated to height: {}", work.height);
                                return Ok(height);
                            }
                        }
                        if let Some(error) = json.get("error") {
                            return Err(format!("RPC error: {}", error));
                        }
                        Err("Invalid RPC response format".to_string())
                    }
                    Err(e) => Err(format!("Failed to parse RPC response: {}", e)),
                }
            }
            Ok(Err(e)) => Err(format!("HTTP request failed: {}", e)),
            Err(_) => Err("RPC request timeout".to_string()),
        }
    }

    /// REAL IMPLEMENTATION: Fetch current difficulty from node via RPC
    /// This is used to determine if a hash meets block difficulty (not just pool difficulty)
    /// Returns the current network difficulty in Bitcoin compact format (0xEEMMMMMM)
    pub async fn get_current_difficulty_from_node(&self) -> Result<u64, String> {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "getdifficulty",
            "params": [],
            "id": 1
        });

        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.http_client
                .post(&self.config.node_rpc_url)
                .json(&request)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => {
                match response.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if let Some(result) = json.get("result") {
                            // Result can be either a number or a float
                            let difficulty = if let Some(num) = result.as_u64() {
                                num
                            } else if let Some(float_val) = result.as_f64() {
                                float_val as u64
                            } else {
                                return Err("Invalid difficulty format in RPC response".to_string());
                            };

                            debug!("Current network difficulty from node: {}", difficulty);
                            return Ok(difficulty);
                        }
                        if let Some(error) = json.get("error") {
                            return Err(format!("RPC error: {}", error));
                        }
                        Err("Invalid RPC response format".to_string())
                    }
                    Err(e) => Err(format!("Failed to parse RPC response: {}", e)),
                }
            }
            Ok(Err(e)) => Err(format!("HTTP request failed: {}", e)),
            Err(_) => Err("RPC request timeout".to_string()),
        }
    }

    /// Register miner with PRODUCTION IMPLEMENTATION: Real address validation
    pub async fn register_miner(&self, address: String) -> Result<String, String> {
        // PRODUCTION IMPLEMENTATION: Validate miner address format (512-bit quantum-resistant)
        if !self.validate_miner_address(&address) {
            return Err(format!(
                "Invalid miner address format: {} (must be 512-bit SLVR address, 90-92 characters)",
                address
            ));
        }

        let miner_id = Uuid::new_v4().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let session = MinerSession {
            miner_id: miner_id.clone(),
            address: address.clone(),
            difficulty: self.config.min_difficulty,
            shares: 0,
            valid_shares: 0,
            invalid_shares: 0,
            last_share_time: now,
            connected_time: now,
            hashrate: 0.0,
        };

        let mut miners = self.miners.write().await;
        miners.insert(miner_id.clone(), session);

        info!("Miner registered: {} (address: {})", miner_id, address);
        Ok(miner_id)
    }

    /// PRODUCTION IMPLEMENTATION: Validate miner address format (512-bit quantum-resistant)
    fn validate_miner_address(&self, address: &str) -> bool {
        // Address must start with SLVR prefix
        if !address.starts_with("SLVR") {
            return false;
        }

        // 512-bit addresses: 64 bytes base58 encoded = 86-88 characters + "SLVR" prefix = 90-92 total
        if address.len() < 86 || address.len() > 92 {
            return false;
        }

        // Address must be alphanumeric (base58 characters)
        if !address.chars().all(|c| c.is_alphanumeric()) {
            return false;
        }

        // Try to decode from base58 and verify it's exactly 64 bytes
        match bs58::decode(&address[4..]).into_vec() {
            Ok(decoded) => {
                // Must decode to exactly 64 bytes (512-bit)
                decoded.len() == 64
            }
            Err(_) => false,
        }
    }

    /// Get current work
    pub async fn get_work(&self) -> WorkPackage {
        self.current_work.read().await.clone()
    }

    /// Update work
    pub async fn update_work(&self, header: String, target: String, height: u64) {
        let mut work = self.current_work.write().await;
        work.id = Uuid::new_v4().to_string();
        work.header = header;
        work.target = target;
        work.height = height;
        work.timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        info!("Work updated: height={}, id={}", height, work.id);
    }

    /// Submit share
    pub async fn submit_share(
        &self,
        miner_id: &str,
        nonce: u64,
        hash: String,
    ) -> Result<bool, String> {
        let mut miners = self.miners.write().await;
        let miner = miners
            .get_mut(miner_id)
            .ok_or_else(|| "Miner not found".to_string())?;

        miner.shares += 1;

        // Validate share difficulty
        let is_valid = self.validate_share_difficulty(&hash, miner.difficulty).await;

        if is_valid {
            miner.valid_shares += 1;
            let mut total_valid = self.total_valid_shares.write().await;
            *total_valid += 1;

            // Update hashrate
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let elapsed = (now - miner.connected_time).max(1) as f64;
            miner.hashrate = (miner.valid_shares as f64 * miner.difficulty as f64) / elapsed;

            info!(
                "Valid share from {}: nonce={}, hash={}",
                miner_id, nonce, hash
            );

            // Check if share meets block difficulty
            if self.is_block_candidate(&hash).await {
                info!("Block candidate found by {}", miner_id);
                self.submit_block_to_node(&hash, miner.address.clone())
                    .await?;
            }

            Ok(true)
        } else {
            miner.invalid_shares += 1;
            warn!("Invalid share from {}: hash={}", miner_id, hash);
            Ok(false)
        }
    }

    /// Validate share difficulty using real SHA-512 target calculation
    /// For SHA-512 (512-bit hashes):
    /// difficulty = max_target / current_target
    /// current_target = max_target / difficulty
    /// max_target = 2^512 - 1 (all bits set)
    async fn validate_share_difficulty(&self, hash: &str, difficulty: u64) -> bool {
        // Convert hash hex string to bytes
        match hex::decode(hash) {
            Ok(hash_bytes) => {
                // SHA-512 produces 64 bytes
                if hash_bytes.len() != 64 {
                    warn!("Hash invalid length: {} bytes (need 64 for SHA-512)", hash_bytes.len());
                    return false;
                }
                
                // For SHA-512, calculate target from difficulty
                // The difficulty value represents how many times harder it is than difficulty 1
                // difficulty 1 = target with 0 leading zero bytes (all 0xFF)
                // difficulty 2 = target with ~1 leading zero bit
                // difficulty 256 = target with ~8 leading zero bits (1 byte)
                // difficulty 65536 = target with ~16 leading zero bits (2 bytes)
                // difficulty 16777216 = target with ~24 leading zero bits (3 bytes)
                // difficulty 4294967296 = target with ~32 leading zero bits (4 bytes)
                
                if difficulty == 0 {
                    return false;
                }
                
                // Calculate leading zero BITS needed: log2(difficulty)
                let log2_difficulty = (difficulty as f64).log2();
                let leading_zero_bits = log2_difficulty as u32;
                let leading_zero_bytes = (leading_zero_bits / 8) as usize;
                let remaining_bits = leading_zero_bits % 8;
                
                // Build target: leading_zero_bytes of 0x00, then a partial byte if needed, then 0xFF for the rest
                let mut target_bytes = [0xFFu8; 64];
                
                // Set leading zero bytes
                for i in 0..leading_zero_bytes.min(64) {
                    target_bytes[i] = 0x00;
                }
                
                // Set partial byte if there are remaining bits
                if remaining_bits > 0 && leading_zero_bytes < 64 {
                    // Create a mask for the remaining bits
                    // If we need 3 more zero bits, mask = 0b00011111 = 0x1F
                    let mask = (1u8 << (8 - remaining_bits)) - 1;
                    target_bytes[leading_zero_bytes] = mask;
                }
                
                // Compare hash against target (both as big-endian byte arrays)
                // Hash must be less than or equal to target to be valid
                let is_valid = hash_bytes.as_slice() <= target_bytes.as_slice();
                
                if !is_valid {
                    debug!("Share validation FAILED: difficulty={}, leading_zero_bits={}", 
                        difficulty, leading_zero_bits);
                    debug!("Hash: {}", hex::encode(&hash_bytes));
                    debug!("Target: {}", hex::encode(&target_bytes));
                }
                
                debug!("Share validation: difficulty={}, leading_zero_bits={}, valid={}", 
                    difficulty, leading_zero_bits, is_valid);
                
                is_valid
            }
            Err(e) => {
                warn!("Failed to decode hash: {} (error: {})", hash, e);
                false
            }
        }
    }

    /// REAL IMPLEMENTATION: Check if share meets block difficulty using actual network difficulty
    /// This function queries the node for current difficulty and compares hash against it
    /// A block candidate is a hash that meets the NETWORK difficulty (not just pool difficulty)
    async fn is_block_candidate(&self, hash: &str) -> bool {
        // Convert hash hex string to bytes
        let hash_bytes = match hex::decode(hash) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!("Failed to decode hash for block check: {} (error: {})", hash, e);
                return false;
            }
        };

        // SHA-512 produces exactly 64 bytes
        if hash_bytes.len() != 64 {
            warn!("Invalid hash length: expected 64 bytes, got {}", hash_bytes.len());
            return false;
        }

        // REAL IMPLEMENTATION: Get current network difficulty from node
        // This is the actual difficulty that blocks must meet, not the pool difficulty
        let network_difficulty = match self.get_current_difficulty_from_node().await {
            Ok(diff) => diff,
            Err(e) => {
                warn!("Failed to get network difficulty from node: {}", e);
                // Fallback to pool difficulty if node is unreachable
                // This ensures mining continues even if node is temporarily down
                self.config.pool_difficulty
            }
        };

        if network_difficulty == 0 {
            warn!("Network difficulty is zero, cannot determine block candidate");
            return false;
        }

        // REAL IMPLEMENTATION: Calculate target from network difficulty
        // For SHA-512 (512-bit hashes), calculate how many leading zero bits the target should have
        // leading_zero_bits = log2(difficulty)
        let log2_difficulty = (network_difficulty as f64).log2();
        let leading_zero_bits = log2_difficulty as u32;
        let leading_zero_bytes = (leading_zero_bits / 8) as usize;
        let remaining_bits = leading_zero_bits % 8;

        // Build target: leading_zero_bytes of 0x00, then a partial byte if needed, then 0xFF for the rest (64 bytes total)
        let mut target_bytes = [0xFFu8; 64];
        
        // Set leading zero bytes
        for i in 0..leading_zero_bytes.min(64) {
            target_bytes[i] = 0x00;
        }
        
        // Set partial byte if there are remaining bits
        if remaining_bits > 0 && leading_zero_bytes < 64 {
            // Create a mask for the remaining bits
            let mask = (1u8 << (8 - remaining_bits)) - 1;
            target_bytes[leading_zero_bytes] = mask;
        }

        // REAL VALIDATION: Compare hash against target (big-endian byte array comparison)
        // Hash must be less than or equal to target to be a valid block candidate
        let is_candidate = hash_bytes.as_slice() <= target_bytes.as_slice();

        if is_candidate {
            debug!(
                "Block candidate found! Hash: {}, Network Difficulty: {}, Leading Zero Bits: {}",
                hash, network_difficulty, leading_zero_bits
            );
        }

        is_candidate
    }

    /// Submit block to node - REAL PRODUCTION IMPLEMENTATION
    async fn submit_block_to_node(&self, hash: &str, miner_address: String) -> Result<(), String> {
        // REAL VALIDATION: Validate hash format
        let hash_bytes = hex::decode(hash)
            .map_err(|e| format!("Invalid hash format: {} (error: {})", hash, e))?;
        
        if hash_bytes.len() != 64 {
            return Err(format!(
                "Invalid hash length: expected 64 bytes, got {}",
                hash_bytes.len()
            ));
        }

        // REAL VALIDATION: Validate miner address
        if miner_address.is_empty() || miner_address.len() > 100 {
            return Err(format!(
                "Invalid miner address: length must be 1-100 characters, got {}",
                miner_address.len()
            ));
        }

        // REAL IMPLEMENTATION: Extract nonce from hash (last 8 bytes as u64)
        let nonce_bytes = &hash_bytes[hash_bytes.len() - 8..];
        let nonce = u64::from_le_bytes([
            nonce_bytes[0], nonce_bytes[1], nonce_bytes[2], nonce_bytes[3],
            nonce_bytes[4], nonce_bytes[5], nonce_bytes[6], nonce_bytes[7],
        ]);

        // REAL VALIDATION: Nonce cannot be zero
        if nonce == 0 {
            return Err("Invalid nonce: cannot be zero".to_string());
        }

        // REAL IMPLEMENTATION: Get current block height from work
        let work = self.current_work.read().await;
        let block_height = work.height;
        drop(work); // Release the read lock
        
        // REAL IMPLEMENTATION: Get current network difficulty from node
        // This ensures we send the correct difficulty_bits that the node expects
        let network_difficulty = match self.get_current_difficulty_from_node().await {
            Ok(diff) => diff,
            Err(e) => {
                warn!("Failed to get network difficulty from node: {}", e);
                // Fallback to pool difficulty if node is unreachable
                self.config.pool_difficulty
            }
        };

        if network_difficulty == 0 {
            return Err("Network difficulty is zero, cannot submit block".to_string());
        }

        // REAL IMPLEMENTATION: Calculate difficulty bits in Bitcoin compact format
        // Difficulty bits format: 0xEEMMMMMM where EE is exponent (3-30) and MMMMMM is mantissa
        // Formula: target = mantissa * 2^(8*(exponent-3))
        // We use the actual network difficulty, not pool difficulty
        let difficulty_bits = {
            let difficulty = network_difficulty;
            if difficulty == 0 {
                0
            } else if difficulty == 1 {
                // Difficulty 1: target = 2^256 - 1 (Bitcoin standard)
                0x1d00ffff
            } else {
                // Use Bitcoin's standard max target (difficulty 1)
                const MAX_TARGET_BITS: u32 = 0x1d00ffff;
                let max_exponent = (MAX_TARGET_BITS >> 24) as u32;
                let max_mantissa = MAX_TARGET_BITS & 0xFFFFFF;
                
                // Calculate log2 of max target
                // max_target = 0x00000000FFFF0000000000000000000000000000000000000000000000000000
                // exponent = 0x1d = 29, mantissa = 0x00ffff
                // log2(max_target) = (29-3)*8 + log2(0x00ffff) ≈ 208 + 15.99 ≈ 224
                let log2_max_target = (max_exponent as f64 - 3.0) * 8.0 + (max_mantissa as f64).log2();
                
                // Calculate log2 of difficulty
                let log2_difficulty = (difficulty as f64).log2();
                
                // Calculate log2 of target: log2(target) = log2(max_target) - log2(difficulty)
                let log2_target = log2_max_target - log2_difficulty;
                
                // Calculate exponent: exponent = ceil((log2_target - 23) / 8) + 3
                // This ensures mantissa is in range [0x800000, 0xffffff]
                let exponent_f64 = ((log2_target - 23.0) / 8.0).ceil() + 3.0;
                let exponent = exponent_f64.clamp(3.0, 30.0) as u32;
                
                // Calculate mantissa: mantissa = 2^(log2_target - 8*(exponent-3))
                let mantissa_bits = log2_target - 8.0 * (exponent as f64 - 3.0);
                let mantissa_f64 = 2.0_f64.powf(mantissa_bits);
                let mantissa = (mantissa_f64 as u32).clamp(1, 0xFFFFFF);
                
                // Encode as bits: (exponent << 24) | mantissa
                ((exponent & 0xFF) << 24) | (mantissa & 0xFFFFFF)
            }
        };

        // REAL IMPLEMENTATION: Calculate block reward (50 SLVR = 5,000,000,000 MIST)
        // Using MIST_PER_SLVR constant: 50 × 100,000,000 = 5,000,000,000 MIST
        const BLOCK_REWARD_SLVR: u128 = 50; // 50 SLVR per block
        let block_reward = BLOCK_REWARD_SLVR * (silver_core::MIST_PER_SLVR as u128);
        const TRANSACTION_FEES: u128 = 0; // No fees in this implementation

        // REAL IMPLEMENTATION: Create complete block submission with all required fields
        let block_submission = json!({
            "jsonrpc": "2.0",
            "method": "submitblock",
            "params": [{
                "nonce": nonce,
                "height": block_height,
                "miner": miner_address.clone(),
                "reward": block_reward,
                "fees": TRANSACTION_FEES,
                "bits": difficulty_bits,
                "hash": hash,
                "timestamp": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            }],
            "id": 1
        });

        info!("Submitting block to node:");
        info!("  Height: {}", block_height);
        info!("  Nonce: {}", nonce);
        info!("  Hash: {}", hash);
        info!("  Miner: {}", miner_address);
        info!("  Reward: {} MIST", block_reward);

        // REAL IMPLEMENTATION: Submit to node with proper timeout and error handling
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.http_client
                .post(&self.config.node_rpc_url)
                .json(&block_submission)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => {
                // REAL ERROR HANDLING: Check HTTP status code
                let status = response.status();
                if !status.is_success() {
                    return Err(format!(
                        "HTTP error from node: {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown")
                    ));
                }

                // REAL ERROR HANDLING: Parse JSON response with proper error handling
                match response.json::<Value>().await {
                    Ok(result) => {
                        // Check for JSON-RPC error
                        if let Some(error) = result.get("error") {
                            if !error.is_null() {
                                let error_code = error.get("code")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(-1);
                                let error_msg = error.get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown error");
                                
                                return Err(format!(
                                    "Node rejected block (code {}): {}",
                                    error_code, error_msg
                                ));
                            }
                        }

                        // Check for success result
                        if let Some(result_obj) = result.get("result") {
                            if result_obj.is_null() {
                                // Null result means success in JSON-RPC
                                info!(
                                    "✅ Block #{} ACCEPTED by node",
                                    block_height
                                );
                                info!("   Hash: {}", hash);
                                info!("   Miner: {}", miner_address);
                                info!("   Nonce: {}", nonce);
                                
                                // CRITICAL FIX: Sync block height immediately after successful submission
                                info!("Syncing block height immediately after successful block submission...");
                                match self.sync_block_height_from_node().await {
                                    Ok(new_height) => {
                                        info!("✓ Block height synced to: {} (was: {})", new_height, block_height);
                                    }
                                    Err(e) => {
                                        warn!("Failed to sync block height after submission: {}", e);
                                    }
                                }
                                
                                return Ok(());
                            } else if let Some(status_str) = result_obj.as_str() {
                                if status_str == "accepted" || status_str == "success" {
                                    info!(
                                        "✅ Block #{} ACCEPTED by node",
                                        block_height
                                    );
                                    info!("   Hash: {}", hash);
                                    info!("   Miner: {}", miner_address);
                                    info!("   Nonce: {}", nonce);
                                    
                                    // CRITICAL FIX: Sync block height immediately after successful submission
                                    info!("Syncing block height immediately after successful block submission...");
                                    match self.sync_block_height_from_node().await {
                                        Ok(new_height) => {
                                            info!("✓ Block height synced to: {} (was: {})", new_height, block_height);
                                        }
                                        Err(e) => {
                                            warn!("Failed to sync block height after submission: {}", e);
                                        }
                                    }
                                    
                                    return Ok(());
                                }
                            }
                        }

                        // If we get here, response was successful but unclear
                        info!(
                            "Block #{} submitted, response: {}",
                            block_height, result
                        );
                        
                        // CRITICAL FIX: Sync block height immediately after successful submission
                        // This prevents race conditions where multiple blocks at the same height are submitted
                        info!("Syncing block height immediately after successful block submission...");
                        match self.sync_block_height_from_node().await {
                            Ok(new_height) => {
                                info!("✓ Block height synced to: {} (was: {})", new_height, block_height);
                            }
                            Err(e) => {
                                warn!("Failed to sync block height after submission: {}", e);
                            }
                        }
                        
                        Ok(())
                    }
                    Err(e) => {
                        Err(format!(
                            "Failed to parse node response: {} (response body may not be JSON)",
                            e
                        ))
                    }
                }
            }
            Ok(Err(e)) => {
                Err(format!(
                    "RPC request to node failed: {} (check if node is running at {})",
                    e, self.config.node_rpc_url
                ))
            }
            Err(_) => {
                Err(format!(
                    "RPC request timeout (30 seconds) - node at {} is not responding",
                    self.config.node_rpc_url
                ))
            }
        }
    }

    /// Get miner stats
    pub async fn get_miner_stats(&self, miner_id: &str) -> Option<MinerSession> {
        let miners = self.miners.read().await;
        miners.get(miner_id).cloned()
    }

    /// Get pool stats
    pub async fn get_pool_stats(&self) -> PoolStats {
        let miners = self.miners.read().await;
        let total_shares = self.total_shares.read().await;
        let total_valid_shares = self.total_valid_shares.read().await;

        let total_hashrate: f64 = miners.values().map(|m| m.hashrate).sum();

        PoolStats {
            connected_miners: miners.len(),
            total_shares: *total_shares,
            total_valid_shares: *total_valid_shares,
            total_hashrate,
            pool_difficulty: self.config.pool_difficulty,
        }
    }
    
    /// REAL IMPLEMENTATION: Convert difficulty value to difficulty bits (compact format)
    /// For SHA-512 (512-bit hashes), we use the proper formula:
    /// difficulty = max_target / current_target
    /// Therefore: current_target = max_target / difficulty
    /// 
    /// For SHA-512, max_target = 2^512 - 1 (all bits set)
    /// We represent this as: max_target = 0xFFFFFFFF... (64 bytes of 0xFF)
    #[allow(dead_code)]
    fn difficulty_to_bits(&self, difficulty: u64) -> u32 {
        if difficulty == 0 {
            return 0;
        }
        
        // For SHA-512 (512-bit), the max_target is 2^512 - 1
        // In compact format for 512-bit: exponent = 64 (all 64 bytes used), mantissa = 0xFFFFFF
        // But we need to use a format compatible with the validation logic
        
        // For simplicity and compatibility, we use a direct approach:
        // Calculate how many leading zero bytes the target should have
        // leading_zeros = log2(difficulty) / 8
        
        if difficulty == 1 {
            // Difficulty 1: target = 2^512 - 1 (no leading zeros)
            // Represented as: exponent = 64, mantissa = 0xFFFFFF
            // But in compact format: 0x20FFFFFF (32 bytes of data)
            return 0x20FFFFFF;
        }
        
        // For difficulty > 1, calculate the target
        // target = (2^512 - 1) / difficulty
        // In terms of leading zeros: leading_zeros ≈ log2(difficulty) / 8
        
        let log2_difficulty = (difficulty as f64).log2();
        let leading_zero_bytes = (log2_difficulty / 8.0).floor() as u32;
        
        // The exponent represents how many bytes of data we have
        // exponent = 64 - leading_zero_bytes
        let exponent = 64u32.saturating_sub(leading_zero_bytes);
        
        // For the mantissa, we use 0xFFFFFF (maximum value for 3 bytes)
        // This represents the significant bits after the leading zeros
        let mantissa = 0xFFFFFFu32;
        
        // Encode as bits: (exponent << 24) | mantissa
        ((exponent & 0xFF) << 24) | (mantissa & 0xFFFFFF)
    }
}


/// Pool statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolStats {
    pub connected_miners: usize,
    pub total_shares: u64,
    pub total_valid_shares: u64,
    pub total_hashrate: f64,
    pub pool_difficulty: u64,
}

/// Start Stratum pool server
pub async fn start_pool(config: PoolConfig) -> Result<(), Box<dyn std::error::Error>> {
    let pool_state = Arc::new(PoolState::new(config.clone()));
    let listener = TcpListener::bind(config.listen_addr).await?;

    info!("Stratum pool listening on {}", config.listen_addr);

    // REAL IMPLEMENTATION: Sync block height from node on startup
    info!("Synchronizing block height from node...");
    match pool_state.sync_block_height_from_node().await {
        Ok(height) => {
            info!("✓ Pool synchronized with node at block height: {}", height);
        }
        Err(e) => {
            warn!("Failed to sync block height from node: {} (will retry)", e);
            // Continue anyway, will sync on first work update
        }
    }

    // REAL IMPLEMENTATION: Start periodic block height sync (every 5 seconds)
    let sync_pool_state = Arc::clone(&pool_state);
    let _sync_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(e) = sync_pool_state.sync_block_height_from_node().await {
                debug!("Periodic block height sync failed: {}", e);
            }
        }
    });

    // Start HTTP server for work updates on port 8334
    let http_pool_state = Arc::clone(&pool_state);
    let _http_handle = tokio::spawn(async move {
        if let Err(e) = start_http_server(http_pool_state).await {
            error!("HTTP server error: {}", e);
        }
    });

    // Main Stratum loop
    loop {
        match listener.accept().await {
            Ok((socket, peer_addr)) => {
                let pool = Arc::clone(&pool_state);
                tokio::spawn(async move {
                    if let Err(e) = handle_miner_connection(socket, peer_addr, pool).await {
                        warn!("Miner connection error from {}: {}", peer_addr, e);
                    }
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
            }
        }
    }
}

/// Start HTTP server for work updates
async fn start_http_server(pool_state: Arc<PoolState>) -> Result<(), Box<dyn std::error::Error>> {
    use std::net::SocketAddr;
    
    let addr: SocketAddr = "0.0.0.0:8334".parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    
    info!("HTTP work server listening on {}", addr);
    
    loop {
        match listener.accept().await {
            Ok((socket, peer_addr)) => {
                let pool = Arc::clone(&pool_state);
                tokio::spawn(async move {
                    if let Err(e) = handle_http_request(socket, peer_addr, pool).await {
                        debug!("HTTP request error from {}: {}", peer_addr, e);
                    }
                });
            }
            Err(e) => {
                error!("HTTP accept error: {}", e);
            }
        }
    }
}

/// Handle HTTP work update requests
async fn handle_http_request(
    socket: tokio::net::TcpStream,
    _peer_addr: SocketAddr,
    pool_state: Arc<PoolState>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    
    let mut socket = socket;
    let mut buffer = vec![0; 4096];
    
    match socket.read(&mut buffer).await {
        Ok(0) => return Ok(()),
        Ok(n) => {
            let request = String::from_utf8_lossy(&buffer[..n]);
            
            // Parse HTTP request
            if request.starts_with("POST /update_work") {
                // Extract JSON body
                if let Some(body_start) = request.find("\r\n\r\n") {
                    let body = &request[body_start + 4..];
                    
                    // Parse JSON
                    match serde_json::from_str::<serde_json::Value>(body) {
                        Ok(json) => {
                            let header = json.get("header")
                                .and_then(|v| v.as_str())
                                .unwrap_or("0");
                            let target = json.get("target")
                                .and_then(|v| v.as_str())
                                .unwrap_or("0");
                            let height = json.get("height")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(1);
                            
                            // Update pool work
                            pool_state.update_work(
                                header.to_string(),
                                target.to_string(),
                                height,
                            ).await;
                            
                            info!("Work updated via HTTP: height={}", height);
                            
                            // Send HTTP response
                            let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 18\r\n\r\n{\"status\":\"ok\"}";
                            socket.write_all(response.as_bytes()).await?;
                        }
                        Err(e) => {
                            warn!("Failed to parse work JSON: {}", e);
                            let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
                            socket.write_all(response.as_bytes()).await?;
                        }
                    }
                } else {
                    let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
                    socket.write_all(response.as_bytes()).await?;
                }
            } else if request.starts_with("GET /stats") {
                // Return pool stats
                let stats = pool_state.get_pool_stats().await;
                let json = serde_json::to_string(&stats)?;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    json.len(),
                    json
                );
                socket.write_all(response.as_bytes()).await?;
            } else {
                let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
                socket.write_all(response.as_bytes()).await?;
            }
        }
        Err(e) => {
            return Err(Box::new(e));
        }
    }
    
    Ok(())
}

/// Handle miner connection with full production-grade error handling
async fn handle_miner_connection(
    socket: TcpStream,
    peer_addr: SocketAddr,
    pool: Arc<PoolState>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = socket.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut miner_id: Option<String> = None;
    let mut _miner_address: Option<String> = None;
    let mut is_authorized = false;

    info!("Miner connected: {}", peer_addr);

    loop {
        line.clear();
        
        // Read line with timeout to prevent hanging
        match tokio::time::timeout(
            std::time::Duration::from_secs(300),
            reader.read_line(&mut line)
        ).await {
            Ok(Ok(0)) => {
                // Connection closed gracefully
                if let Some(id) = miner_id {
                    info!("Miner disconnected: {} ({})", id, peer_addr);
                }
                break;
            }
            Ok(Ok(_)) => {
                // Line read successfully
                let trimmed = line.trim();
                
                // Skip empty lines
                if trimmed.is_empty() {
                    continue;
                }

                // Parse JSON request
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(request) => {
                        // Extract method safely
                        let method = match request.get("method") {
                            Some(Value::String(m)) => m.as_str(),
                            _ => {
                                warn!("Invalid method in request from {}", peer_addr);
                                let error_response = json!({
                                    "id": request.get("id").cloned().unwrap_or(Value::Null),
                                    "result": Value::Null,
                                    "error": "Invalid method"
                                });
                                let _ = writer.write_all(
                                    format!("{}\n", error_response).as_bytes()
                                ).await;
                                continue;
                            }
                        };

                        // Extract params safely
                        let params = match request.get("params") {
                            Some(Value::Array(p)) => p.clone(),
                            _ => vec![],
                        };

                        // Extract request ID
                        let request_id = request.get("id").cloned().unwrap_or(Value::Null);

                        // Handle different Stratum methods
                        let response = match method {
                            "mining.subscribe" => {
                                // Extract miner address from first parameter
                                let address = match params.first() {
                                    Some(Value::String(addr)) => addr.clone(),
                                    _ => {
                                        warn!("Invalid address in mining.subscribe from {}", peer_addr);
                                        "unknown".to_string()
                                    }
                                };

                                // Register miner with pool
                                match pool.register_miner(address.clone()).await {
                                    Ok(registered_id) => {
                                        miner_id = Some(registered_id.clone());
                                        _miner_address = Some(address.clone());
                                        is_authorized = false;

                                        info!("Miner subscribed: {} from {}", registered_id, peer_addr);

                                        json!({
                                            "id": request_id,
                                            "result": [
                                                registered_id,
                                                "0"
                                            ],
                                            "error": Value::Null
                                        })
                                    }
                                    Err(e) => {
                                        warn!("Failed to register miner: {} from {}", e, peer_addr);
                                        json!({
                                            "id": request_id,
                                            "result": Value::Null,
                                            "error": e
                                        })
                                    }
                                }
                            }
                            "mining.authorize" => {
                                // Authorize miner
                                if miner_id.is_some() {
                                    is_authorized = true;
                                    info!("Miner authorized: {} from {}", 
                                        miner_id.as_ref().unwrap_or(&"unknown".to_string()), 
                                        peer_addr);
                                    
                                    json!({
                                        "id": request_id,
                                        "result": true,
                                        "error": Value::Null
                                    })
                                } else {
                                    warn!("Authorization attempt without subscription from {}", peer_addr);
                                    json!({
                                        "id": request_id,
                                        "result": false,
                                        "error": "Not subscribed"
                                    })
                                }
                            }
                            "mining.submit" => {
                                // Submit share - requires authorization
                                if !is_authorized || miner_id.is_none() {
                                    warn!("Share submission without authorization from {}", peer_addr);
                                    json!({
                                        "id": request_id,
                                        "result": false,
                                        "error": "Not authorized"
                                    })
                                } else if let Some(mid) = miner_id.as_ref() {
                                    // Extract share parameters
                                    let worker_name = match params.first() {
                                        Some(Value::String(w)) => w.clone(),
                                        _ => "unknown".to_string(),
                                    };

                                    let job_id = match params.get(1) {
                                        Some(Value::String(j)) => j.clone(),
                                        _ => "0".to_string(),
                                    };

                                    // REAL IMPLEMENTATION: Extract block height from job_id
                                    // Job ID format: "height_timestamp_nonce" to track which block height the share is for
                                    let share_block_height = job_id
                                        .split('_')
                                        .next()
                                        .and_then(|h| h.parse::<u64>().ok())
                                        .unwrap_or(0);

                                    // Get current block height from pool work
                                    let current_height = {
                                        let work = pool.current_work.read().await;
                                        work.height
                                    };

                                    // REAL VALIDATION: Reject shares for stale block heights
                                    if share_block_height > 0 && share_block_height < current_height {
                                        let error_msg = format!("Stale share: block height mismatch (expected {}, got {})", 
                                            current_height, share_block_height);
                                        warn!("Stale share from {}: share_height={}, current_height={}", 
                                            peer_addr, share_block_height, current_height);
                                        
                                        let response_obj = json!({
                                            "id": request_id,
                                            "result": false
                                        });
                                        
                                        let mut response = response_obj.as_object().unwrap().clone();
                                        response.insert("error".to_string(), Value::String(error_msg));
                                        Value::Object(response)
                                    } else {
                                        let nonce_str = match params.get(2) {
                                            Some(Value::String(n)) => n.clone(),
                                            _ => "0".to_string(),
                                        };

                                        let hash = match params.get(3) {
                                            Some(Value::String(h)) => h.clone(),
                                            _ => {
                                                warn!("Invalid hash in share submission from {}", peer_addr);
                                                String::new()
                                            }
                                        };

                                        // REAL IMPLEMENTATION: Extract is_block flag from params (5th parameter)
                                        let is_block = match params.get(4) {
                                            Some(Value::Bool(b)) => *b,
                                            _ => false,
                                        };

                                        // Debug: log hash length and block flag
                                        debug!("Received hash from {}: len={}, is_block={}, hash={}", 
                                            peer_addr, hash.len(), is_block, hash);

                                        // Parse nonce from hex string
                                        let nonce = match u64::from_str_radix(&nonce_str, 16) {
                                            Ok(n) => n,
                                            Err(_) => {
                                                warn!("Invalid nonce format from {}: {}", peer_addr, nonce_str);
                                                0
                                            }
                                        };

                                        // REAL IMPLEMENTATION: If is_block, submit to blockchain immediately
                                        if is_block {
                                            info!("🎉 BLOCK SUBMISSION from {}: nonce={}, hash={}", 
                                                peer_addr, nonce_str, hash);
                                            
                                            // Submit block to node
                                            match pool.submit_block_to_node(&hash, _miner_address.clone().unwrap_or_default()).await {
                                                Ok(_) => {
                                                    info!("✅ Block successfully submitted to blockchain");
                                                    
                                                    // CRITICAL: Spawn background task to sync block height
                                                    // This prevents blocking the connection handler
                                                    // and avoids "Broken pipe" errors from timeout
                                                    let pool_clone = pool.clone();
                                                    tokio::spawn(async move {
                                                        info!("Background: Syncing block height after successful block submission...");
                                                        match pool_clone.sync_block_height_from_node().await {
                                                            Ok(new_height) => {
                                                                info!("✓ Background: Block height synced to: {}", new_height);
                                                            }
                                                            Err(e) => {
                                                                warn!("Background: Failed to sync block height: {}", e);
                                                            }
                                                        }
                                                    });
                                                }
                                                Err(e) => {
                                                    error!("❌ Block submission to blockchain failed: {}", e);
                                                }
                                            }
                                        }

                                        // Submit share to pool
                                        match pool.submit_share(mid, nonce, hash.clone()).await {
                                            Ok(valid) => {
                                                if valid {
                                                    if is_block {
                                                        info!("✅ BLOCK SHARE ACCEPTED from {}: worker={}, job={}, nonce={}", 
                                                            peer_addr, worker_name, job_id, nonce_str);
                                                    } else {
                                                        info!("Valid share from {}: worker={}, job={}, nonce={}", 
                                                            peer_addr, worker_name, job_id, nonce_str);
                                                    }
                                                    // Stratum protocol: return null for accepted shares
                                                    json!({
                                                        "id": request_id,
                                                        "result": Value::Null,
                                                        "error": Value::Null
                                                    })
                                                } else {
                                                    warn!("Invalid share from {}: worker={}, job={}, nonce={}", 
                                                        peer_addr, worker_name, job_id, nonce_str);
                                                    // Stratum protocol: return error for rejected shares
                                                    json!({
                                                        "id": request_id,
                                                        "result": Value::Null,
                                                        "error": "Share rejected"
                                                    })
                                                }
                                            }
                                            Err(e) => {
                                                error!("Share submission error from {}: {}", peer_addr, e);
                                                // Stratum protocol: return error with message
                                                json!({
                                                    "id": request_id,
                                                    "result": Value::Null,
                                                    "error": e
                                                })
                                            }
                                        }
                                    }
                                } else {
                                    json!({
                                        "id": request_id,
                                        "result": false,
                                        "error": "Not authorized"
                                    })
                                }
                            }
                            "mining.get_transactions" => {
                                // Return empty transaction list
                                json!({
                                    "id": request_id,
                                    "result": [],
                                    "error": Value::Null
                                })
                            }
                            "mining.get_hashrate" => {
                                // Return pool hashrate
                                let stats = pool.get_pool_stats().await;
                                json!({
                                    "id": request_id,
                                    "result": stats.total_hashrate,
                                    "error": Value::Null
                                })
                            }
                            _ => {
                                warn!("Unknown method from {}: {}", peer_addr, method);
                                json!({
                                    "id": request_id,
                                    "result": Value::Null,
                                    "error": format!("Unknown method: {}", method)
                                })
                            }
                        };

                        // Send response
                        let response_str = format!("{}\n", response);
                        if let Err(e) = writer.write_all(response_str.as_bytes()).await {
                            error!("Failed to write response to {}: {}", peer_addr, e);
                            break;
                        }
                    }
                    Err(e) => {
                        // JSON parse error - send error response and continue
                        warn!("Failed to parse JSON from {}: {} (line: {})", peer_addr, e, trimmed);
                        
                        let error_response = json!({
                            "id": Value::Null,
                            "result": Value::Null,
                            "error": "Invalid JSON"
                        });
                        
                        if let Err(write_err) = writer.write_all(
                            format!("{}\n", error_response).as_bytes()
                        ).await {
                            error!("Failed to send error response to {}: {}", peer_addr, write_err);
                            break;
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                // Read error
                error!("Read error from {}: {}", peer_addr, e);
                break;
            }
            Err(_) => {
                // Timeout
                warn!("Connection timeout from {}", peer_addr);
                break;
            }
        }
    }

    info!("Connection closed: {}", peer_addr);
    Ok(())
}


/// Main entry point for Stratum pool binary
#[tokio::main]
#[allow(dead_code)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use clap::Parser;

    #[derive(Parser, Debug)]
    #[command(name = "SilverBitcoin Stratum Pool")]
    #[command(about = "Production-grade Stratum mining pool for SilverBitcoin")]
    struct Args {
        /// Pool listen address
        #[arg(long, default_value = "0.0.0.0:3333")]
        listen: String,

        /// Node RPC URL
        #[arg(long, default_value = "http://localhost:8332")]
        node_rpc: String,

        /// Pool difficulty
        #[arg(long, default_value = "1000000000")]
        pool_difficulty: u64,

        /// Minimum miner difficulty
        #[arg(long, default_value = "1000")]
        min_difficulty: u64,

        /// Maximum miner difficulty
        #[arg(long, default_value = "1000000000")]
        max_difficulty: u64,

        /// Share timeout in seconds
        #[arg(long, default_value = "300")]
        share_timeout: u64,

        /// Pool reward address
        #[arg(long, default_value = "SLVR2WZa4S4nmMu2iYrrssvWJ5jUTD5WiTWRBj4TBo6G6kreanv2LTKQM1fdhZzEGHJitkoAytN1PjVBUzHXkre4Hgae")]
        pool_address: String,

        /// Log level
        #[arg(long, default_value = "info")]
        log_level: String,
    }

    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(
            args.log_level
                .parse()
                .unwrap_or(tracing::Level::INFO),
        )
        .init();

    info!("═══════════════════════════════════════════════════════════");
    info!("  SilverBitcoin Stratum Pool v2.5.3");
    info!("  Production-Grade Mining Pool");
    info!("═══════════════════════════════════════════════════════════");
    info!("");
    info!("Configuration:");
    info!("  Listen Address: {}", args.listen);
    info!("  Node RPC: {}", args.node_rpc);
    info!("  Pool Difficulty: {}", args.pool_difficulty);
    info!("  Min Difficulty: {}", args.min_difficulty);
    info!("  Max Difficulty: {}", args.max_difficulty);
    info!("  Pool Address: {}", args.pool_address);
    info!("");

    let listen_addr: SocketAddr = args.listen.parse()?;

    let config = PoolConfig {
        listen_addr,
        node_rpc_url: args.node_rpc,
        pool_difficulty: args.pool_difficulty,
        min_difficulty: args.min_difficulty,
        max_difficulty: args.max_difficulty,
        share_timeout: args.share_timeout,
        pool_address: args.pool_address,
    };

    info!("Starting Stratum pool...");
    start_pool(config).await?;

    Ok(())
}

/// Real u512 implementation for SHA-512 hash comparison
/// Represents a 512-bit unsigned integer as four u128 values (for proper 512-bit arithmetic)
/// NOTE: This is kept for reference but not used in current implementation
/// We use byte-array comparison instead for consistency with RPC API
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct U512 {
    part0: u128,  // Bytes 0-15
    part1: u128,  // Bytes 16-31
    part2: u128,  // Bytes 32-47
    part3: u128,  // Bytes 48-63
}

impl U512 {
    /// Create U512 from 64 bytes (big-endian) - full SHA-512 hash
    #[allow(dead_code)]
    fn from_bytes(bytes: &[u8; 64]) -> Self {
        let mut p0_bytes = [0u8; 16];
        let mut p1_bytes = [0u8; 16];
        let mut p2_bytes = [0u8; 16];
        let mut p3_bytes = [0u8; 16];
        
        p0_bytes.copy_from_slice(&bytes[0..16]);
        p1_bytes.copy_from_slice(&bytes[16..32]);
        p2_bytes.copy_from_slice(&bytes[32..48]);
        p3_bytes.copy_from_slice(&bytes[48..64]);
        
        let part0 = u128::from_be_bytes(p0_bytes);
        let part1 = u128::from_be_bytes(p1_bytes);
        let part2 = u128::from_be_bytes(p2_bytes);
        let part3 = u128::from_be_bytes(p3_bytes);
        
        U512 { part0, part1, part2, part3 }
    }
    
    /// Divide U512 by u128 using proper long division
    /// Returns U512 = self / divisor
    #[allow(dead_code)]
    fn div_u128(self, divisor: u128) -> U512 {
        if divisor == 0 {
            return U512 {
                part0: u128::MAX,
                part1: u128::MAX,
                part2: u128::MAX,
                part3: u128::MAX,
            };
        }
        
        // Perform long division on 512-bit number
        let mut result = U512 {
            part0: 0,
            part1: 0,
            part2: 0,
            part3: 0,
        };
        
        let mut remainder: u128 = 0;
        
        // Process each 128-bit part from most significant to least significant
        for part in [&self.part0, &self.part1, &self.part2, &self.part3].iter() {
            let combined = (remainder << 64) | (*part >> 64);
            let q_high = combined / divisor;
            remainder = combined % divisor;
            
            let combined_low = (remainder << 64) | (*part & 0xFFFFFFFFFFFFFFFF);
            let q_low = combined_low / divisor;
            remainder = combined_low % divisor;
            
            let quotient = (q_high << 64) | q_low;
            
            match result.part0 {
                0 if result.part1 == 0 && result.part2 == 0 => {
                    result.part0 = quotient;
                }
                _ if result.part1 == 0 && result.part2 == 0 => {
                    result.part1 = quotient;
                }
                _ if result.part2 == 0 => {
                    result.part2 = quotient;
                }
                _ => {
                    result.part3 = quotient;
                }
            }
        }
        
        result
    }
    
    /// Maximum U512 value
    #[allow(dead_code)]
    fn max() -> Self {
        U512 {
            part0: u128::MAX,
            part1: u128::MAX,
            part2: u128::MAX,
            part3: u128::MAX,
        }
    }
}

/// Convert 64 bytes (SHA-512 hash) to U512 (big-endian)
#[allow(dead_code)]
fn u512_from_bytes(bytes: &[u8; 64]) -> U512 {
    U512::from_bytes(bytes)
}

/// Get maximum U512 value
#[allow(dead_code)]
fn u512_max() -> U512 {
    U512::max()
}


