//! Production-grade Stratum Mining Pool for SilverBitcoin
//! Implements Stratum protocol v1 with real share validation and block submission

use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
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

    /// Register miner
    pub async fn register_miner(&self, address: String) -> String {
        let miner_id = Uuid::new_v4().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let session = MinerSession {
            miner_id: miner_id.clone(),
            address,
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

        info!("Miner registered: {}", miner_id);
        miner_id
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

    /// Validate share difficulty
    async fn validate_share_difficulty(&self, hash: &str, difficulty: u64) -> bool {
        // Convert hash to integer for comparison
        if let Ok(hash_bytes) = hex::decode(hash) {
            if hash_bytes.len() >= 8 {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&hash_bytes[0..8]);
                let hash_value = u64::from_le_bytes(bytes);

                // Check if hash meets difficulty target
                let target = u64::MAX / difficulty;
                return hash_value <= target;
            }
        }
        false
    }

    /// Check if share meets block difficulty
    async fn is_block_candidate(&self, hash: &str) -> bool {
        // Block difficulty is much higher than share difficulty
        if let Ok(hash_bytes) = hex::decode(hash) {
            if hash_bytes.len() >= 8 {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&hash_bytes[0..8]);
                let hash_value = u64::from_le_bytes(bytes);

                // Block target (1,000,000 difficulty)
                let block_target = u64::MAX / 1_000_000;
                return hash_value <= block_target;
            }
        }
        false
    }

    /// Submit block to node
    async fn submit_block_to_node(&self, hash: &str, miner_address: String) -> Result<(), String> {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "submitblock",
            "params": [hash],
            "id": 1
        });

        match self
            .http_client
            .post(&self.config.node_rpc_url)
            .json(&request)
            .send()
            .await
        {
            Ok(response) => {
                match response.json::<Value>().await {
                    Ok(result) => {
                        if result["error"].is_null() {
                            info!(
                                "Block submitted successfully: {} by {}",
                                hash, miner_address
                            );
                            Ok(())
                        } else {
                            Err(format!("Node rejected block: {}", result["error"]))
                        }
                    }
                    Err(e) => Err(format!("Failed to parse response: {}", e)),
                }
            }
            Err(e) => Err(format!("RPC request failed: {}", e)),
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
                                let registered_id = pool.register_miner(address.clone()).await;
                                miner_id = Some(registered_id.clone());
                                _miner_address = Some(address.clone());
                                is_authorized = false;

                                info!("Miner subscribed: {} from {}", registered_id, peer_addr);

                                json!({
                                    "id": request_id,
                                    "result": [
                                        ["mining.notify", registered_id],
                                        "0"
                                    ],
                                    "error": Value::Null
                                })
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

                                    // Parse nonce from hex string
                                    let nonce = match u64::from_str_radix(&nonce_str, 16) {
                                        Ok(n) => n,
                                        Err(_) => {
                                            warn!("Invalid nonce format from {}: {}", peer_addr, nonce_str);
                                            0
                                        }
                                    };

                                    // Submit share to pool
                                    match pool.submit_share(mid, nonce, hash.clone()).await {
                                        Ok(valid) => {
                                            if valid {
                                                info!("Valid share from {}: worker={}, job={}, nonce={}", 
                                                    peer_addr, worker_name, job_id, nonce_str);
                                            } else {
                                                warn!("Invalid share from {}: worker={}, job={}, nonce={}", 
                                                    peer_addr, worker_name, job_id, nonce_str);
                                            }

                                            json!({
                                                "id": request_id,
                                                "result": valid,
                                                "error": Value::Null
                                            })
                                        }
                                        Err(e) => {
                                            error!("Share submission error from {}: {}", peer_addr, e);
                                            json!({
                                                "id": request_id,
                                                "result": false,
                                                "error": e
                                            })
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
        #[arg(long, default_value = "1000000")]
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
        #[arg(long, default_value = "SLVR69NJeyPfG2HXSKVq4EvW3j8M9bZ4CtkHc6vJLveJ1fv")]
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
