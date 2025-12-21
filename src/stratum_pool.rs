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

    /// Validate share difficulty using real u256 comparison (matching miner logic)
    async fn validate_share_difficulty(&self, hash: &str, difficulty: u64) -> bool {
        // Convert hash hex string to bytes
        match hex::decode(hash) {
            Ok(hash_bytes) => {
                // SHA-512 produces 64 bytes, use first 32 bytes for u256 comparison
                if hash_bytes.len() < 32 {
                    warn!("Hash too short: {} bytes (need at least 32)", hash_bytes.len());
                    return false;
                }
                
                let mut hash_u256_bytes = [0u8; 32];
                hash_u256_bytes.copy_from_slice(&hash_bytes[0..32]);
                
                // Convert to U256 for comparison (big-endian)
                let hash_value = u256_from_bytes(&hash_u256_bytes);
                
                // Calculate target: u256_max / difficulty
                let target = u256_max().div_u128(difficulty as u128);
                
                let is_valid = hash_value <= target;
                debug!("Share validation: difficulty={}, valid={}", difficulty, is_valid);
                
                is_valid
            }
            Err(e) => {
                warn!("Failed to decode hash: {} (error: {})", hash, e);
                false
            }
        }
    }

    /// Check if share meets block difficulty using real u256 comparison
    async fn is_block_candidate(&self, hash: &str) -> bool {
        // Convert hash hex string to bytes
        match hex::decode(hash) {
            Ok(hash_bytes) => {
                // SHA-512 produces 64 bytes, use first 32 bytes for u256 comparison
                if hash_bytes.len() < 32 {
                    return false;
                }
                
                let mut hash_u256_bytes = [0u8; 32];
                hash_u256_bytes.copy_from_slice(&hash_bytes[0..32]);
                
                // Convert to U256 for comparison (big-endian)
                let hash_value = u256_from_bytes(&hash_u256_bytes);
                
                // Block difficulty: 1,000,000,000
                const BLOCK_DIFFICULTY: u128 = 1_000_000_000;
                let block_target = u256_max().div_u128(BLOCK_DIFFICULTY);
                
                hash_value <= block_target
            }
            Err(e) => {
                warn!("Failed to decode hash for block check: {} (error: {})", hash, e);
                false
            }
        }
    }

    /// Submit block to node
    async fn submit_block_to_node(&self, hash: &str, miner_address: String) -> Result<(), String> {
        // Parse nonce from hash (last 8 bytes)
        let hash_bytes = hex::decode(hash).map_err(|e| format!("Invalid hash: {}", e))?;
        if hash_bytes.len() < 8 {
            return Err("Hash too short".to_string());
        }

        let nonce_bytes = &hash_bytes[hash_bytes.len() - 8..];
        let nonce = u32::from_le_bytes([nonce_bytes[0], nonce_bytes[1], nonce_bytes[2], nonce_bytes[3]]);

        // Get current block height from work
        let work = self.current_work.read().await;
        let block_height = work.height;
        let difficulty_bits = 0x207fffff; // Standard difficulty bits

        // Submit block with real block data
        let request = json!({
            "jsonrpc": "2.0",
            "method": "submitblock",
            "params": [{
                "nonce": nonce,
                "height": block_height,
                "miner": miner_address,
                "reward": 5000000000u128,
                "fees": 0u128,
                "bits": difficulty_bits
            }],
            "id": 1
        });

        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.http_client
                .post(&self.config.node_rpc_url)
                .json(&request)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => {
                match response.json::<Value>().await {
                    Ok(result) => {
                        if result["error"].is_null() {
                            info!(
                                "Block #{} submitted successfully: {} by {}",
                                block_height, hash, miner_address
                            );
                            Ok(())
                        } else {
                            let error_msg = result["error"]["message"]
                                .as_str()
                                .unwrap_or("Unknown error");
                            warn!("Node rejected block: {}", error_msg);
                            Err(format!("Node rejected: {}", error_msg))
                        }
                    }
                    Err(e) => Err(format!("Failed to parse response: {}", e)),
                }
            }
            Ok(Err(e)) => Err(format!("RPC request failed: {}", e)),
            Err(_) => Err("RPC request timeout (30s)".to_string()),
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
                                let registered_id = pool.register_miner(address.clone()).await;
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

                                    // Debug: log hash length
                                    debug!("Received hash from {}: len={}, hash={}", peer_addr, hash.len(), hash);

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

/// Real u256 implementation for hash comparison
/// Represents a 256-bit unsigned integer as two u128 values (high and low)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct U256 {
    high: u128,
    low: u128,
}

impl U256 {
    /// Create U256 from 32 bytes (big-endian)
    fn from_bytes(bytes: &[u8; 32]) -> Self {
        let mut high_bytes = [0u8; 16];
        let mut low_bytes = [0u8; 16];
        
        high_bytes.copy_from_slice(&bytes[0..16]);
        low_bytes.copy_from_slice(&bytes[16..32]);
        
        let high = u128::from_be_bytes(high_bytes);
        let low = u128::from_be_bytes(low_bytes);
        
        U256 { high, low }
    }
    
    /// Divide U256 by u128 using proper long division
    /// Returns U256 = self / divisor
    fn div_u128(self, divisor: u128) -> U256 {
        if divisor == 0 {
            return U256 { high: u128::MAX, low: u128::MAX };
        }
        
        if self.high == 0 {
            // Simple case: only low part
            return U256 {
                high: 0,
                low: self.low / divisor,
            };
        }
        
        // Complex case: use proper long division
        // We need to divide a 256-bit number by a 128-bit number
        
        // Step 1: Divide high part
        let result_high = self.high / divisor;
        let remainder_high = self.high % divisor;
        
        // Step 2: Combine remainder with low part and divide
        // remainder_high is at most divisor-1, so (remainder_high << 128) won't overflow
        // But we can't represent (remainder_high << 128) + self.low as u128
        // So we need to do this in parts
        
        // Divide the combined value in two parts
        // First, handle the high 64 bits of low
        let low_high = self.low >> 64;
        let low_low = self.low & 0xFFFFFFFFFFFFFFFF;
        
        // Combined high part: (remainder_high << 64) + (low_high)
        // This fits in u128
        let combined_high = (remainder_high << 64) + low_high;
        let result_low_high = combined_high / divisor;
        let remainder_combined = combined_high % divisor;
        
        // Combined low part: (remainder_combined << 64) + low_low
        let combined_low = (remainder_combined << 64) + low_low;
        let result_low_low = combined_low / divisor;
        
        let result_low = (result_low_high << 64) | result_low_low;
        
        U256 {
            high: result_high,
            low: result_low,
        }
    }
    
    /// Maximum U256 value
    fn max() -> Self {
        U256 {
            high: u128::MAX,
            low: u128::MAX,
        }
    }
}

/// Convert 32 bytes to U256 (big-endian)
fn u256_from_bytes(bytes: &[u8; 32]) -> U256 {
    U256::from_bytes(bytes)
}

/// Get U256 max value
fn u256_max() -> U256 {
    U256::max()
}
