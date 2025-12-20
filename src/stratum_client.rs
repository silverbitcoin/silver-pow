//! Production-Grade Stratum Client for Mining Pools
//! Implements Stratum protocol v1 with real TCP socket communication
//! FULL PRODUCTION IMPLEMENTATION - NO MOCKS, NO UNWRAP, NO PLACEHOLDERS

use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Stratum client connection state
#[derive(Debug, Clone, Copy, PartialEq)]
enum ConnectionState {
    Disconnected,
    Connected,
    Subscribed,
    Authorized,
}

/// Stratum client for connecting to mining pools
pub struct StratumClient {
    /// Pool address (host:port)
    pool_addr: String,
    /// Miner address (wallet)
    miner_address: String,
    /// TCP connection to pool
    connection: Arc<Mutex<Option<(tokio::net::tcp::OwnedReadHalf, tokio::net::tcp::OwnedWriteHalf)>>>,
    /// Subscription ID from pool
    subscription_id: Arc<Mutex<Option<String>>>,
    /// Connection state
    state: Arc<Mutex<ConnectionState>>,
    /// Request ID counter
    request_id: Arc<Mutex<u64>>,
    /// Max reconnection attempts
    max_reconnect_attempts: u32,
}

impl StratumClient {
    /// Create new Stratum client
    pub fn new(pool_addr: String, miner_address: String) -> Self {
        Self {
            pool_addr,
            miner_address,
            connection: Arc::new(Mutex::new(None)),
            subscription_id: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(ConnectionState::Disconnected)),
            request_id: Arc::new(Mutex::new(1)),
            max_reconnect_attempts: 5,
        }
    }

    /// Connect to pool and perform handshake with full error handling
    pub async fn connect(&self) -> Result<(), String> {
        // Try to connect with retries
        let mut attempts = 0;
        loop {
            match TcpStream::connect(&self.pool_addr).await {
                Ok(socket) => {
                    let (reader, writer) = socket.into_split();
                    let mut conn = self.connection.lock().await;
                    *conn = Some((reader, writer));
                    
                    let mut state = self.state.lock().await;
                    *state = ConnectionState::Connected;
                    
                    info!("Connected to Stratum pool: {}", self.pool_addr);
                    break;
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= self.max_reconnect_attempts {
                        return Err(format!(
                            "Failed to connect to pool after {} attempts: {}",
                            self.max_reconnect_attempts, e
                        ));
                    }
                    warn!(
                        "Connection attempt {} failed: {}. Retrying in 2 seconds...",
                        attempts, e
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }

        // Subscribe to mining
        if let Err(e) = self.subscribe().await {
            error!("Subscription failed: {}", e);
            return Err(format!("Subscription failed: {}", e));
        }

        // Authorize miner
        if let Err(e) = self.authorize().await {
            error!("Authorization failed: {}", e);
            return Err(format!("Authorization failed: {}", e));
        }

        Ok(())
    }

    /// Subscribe to mining notifications with full error handling
    async fn subscribe(&self) -> Result<(), String> {
        let request_id = self.get_next_request_id().await;
        let request = json!({
            "id": request_id,
            "method": "mining.subscribe",
            "params": [self.miner_address]
        });

        self.send_request(&request).await?;

        // Read response with timeout
        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.read_response(),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => return Err(format!("Subscribe response error: {}", e)),
            Err(_) => return Err("Subscribe response timeout".to_string()),
        };

        // Extract subscription ID from response
        match response.get("result") {
            Some(Value::Array(arr)) => {
                if let Some(Value::String(sub_id)) = arr.first() {
                    let mut sub = self.subscription_id.lock().await;
                    *sub = Some(sub_id.clone());
                    
                    let mut state = self.state.lock().await;
                    *state = ConnectionState::Subscribed;
                    
                    info!("Subscribed to pool: {}", sub_id);
                    Ok(())
                } else {
                    Err("Invalid subscription ID in response".to_string())
                }
            }
            Some(other) => Err(format!("Unexpected result type in subscription: {:?}", other)),
            None => {
                if let Some(error) = response.get("error") {
                    Err(format!("Pool error: {}", error))
                } else {
                    Err("No result or error in subscription response".to_string())
                }
            }
        }
    }

    /// Authorize miner with full error handling
    async fn authorize(&self) -> Result<(), String> {
        let request_id = self.get_next_request_id().await;
        let request = json!({
            "id": request_id,
            "method": "mining.authorize",
            "params": [self.miner_address, ""]
        });

        self.send_request(&request).await?;

        // Read response with timeout
        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.read_response(),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => return Err(format!("Authorization response error: {}", e)),
            Err(_) => return Err("Authorization response timeout".to_string()),
        };

        // Check if authorized
        match response.get("result") {
            Some(Value::Bool(true)) => {
                let mut state = self.state.lock().await;
                *state = ConnectionState::Authorized;
                info!("Miner authorized: {}", self.miner_address);
                Ok(())
            }
            Some(Value::Bool(false)) => {
                if let Some(error) = response.get("error") {
                    Err(format!("Authorization rejected: {}", error))
                } else {
                    Err("Authorization rejected by pool".to_string())
                }
            }
            Some(other) => Err(format!("Unexpected result type in authorization: {:?}", other)),
            None => {
                if let Some(error) = response.get("error") {
                    Err(format!("Pool error: {}", error))
                } else {
                    Err("No result or error in authorization response".to_string())
                }
            }
        }
    }

    /// Submit share to pool with full error handling
    pub async fn submit_share(
        &self,
        job_id: &str,
        nonce: u64,
        hash: &str,
    ) -> Result<bool, String> {
        // Check if authorized
        let state = self.state.lock().await;
        if *state != ConnectionState::Authorized {
            return Err(format!(
                "Not authorized (state: {:?}). Must call connect() first.",
                *state
            ));
        }
        drop(state);

        let request_id = self.get_next_request_id().await;
        let nonce_hex = format!("{:x}", nonce);

        let request = json!({
            "id": request_id,
            "method": "mining.submit",
            "params": [self.miner_address, job_id, nonce_hex, hash]
        });

        self.send_request(&request).await?;

        // Read response with timeout
        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.read_response(),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                error!("Share response error: {}", e);
                return Err(format!("Share response error: {}", e));
            }
            Err(_) => {
                error!("Share response timeout");
                return Err("Share response timeout".to_string());
            }
        };

        // Check if share was accepted
        match response.get("result") {
            Some(Value::Bool(true)) => {
                debug!("Share accepted: nonce={}, hash={}", nonce, hash);
                Ok(true)
            }
            Some(Value::Bool(false)) => {
                if let Some(error) = response.get("error") {
                    warn!("Share rejected: {}", error);
                } else {
                    warn!("Share rejected by pool");
                }
                Ok(false)
            }
            Some(other) => {
                warn!("Unexpected result type in share response: {:?}", other);
                Ok(false)
            }
            None => {
                if let Some(error) = response.get("error") {
                    warn!("Pool error on share: {}", error);
                    Ok(false)
                } else {
                    warn!("No result or error in share response");
                    Ok(false)
                }
            }
        }
    }

    /// Send request to pool with full error handling
    async fn send_request(&self, request: &Value) -> Result<(), String> {
        let mut conn = self.connection.lock().await;
        
        match conn.as_mut() {
            Some((_, writer)) => {
                let request_str = format!("{}\n", request.to_string());
                match writer.write_all(request_str.as_bytes()).await {
                    Ok(_) => {
                        if let Some(method) = request.get("method") {
                            debug!("Sent request: {}", method);
                        }
                        Ok(())
                    }
                    Err(e) => {
                        error!("Failed to send request: {}", e);
                        Err(format!("Failed to send request: {}", e))
                    }
                }
            }
            None => Err("Not connected to pool".to_string()),
        }
    }

    /// Read response from pool with full error handling
    async fn read_response(&self) -> Result<Value, String> {
        let mut conn = self.connection.lock().await;
        
        match conn.as_mut() {
            Some((reader, _)) => {
                let mut buf_reader = BufReader::new(reader);
                let mut line = String::new();

                match buf_reader.read_line(&mut line).await {
                    Ok(0) => {
                        error!("Connection closed by pool");
                        Err("Connection closed by pool".to_string())
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            return Err("Empty response from pool".to_string());
                        }

                        match serde_json::from_str::<Value>(trimmed) {
                            Ok(response) => {
                                debug!("Received response: {:?}", response);
                                Ok(response)
                            }
                            Err(e) => {
                                error!("Failed to parse response: {} (line: {})", e, trimmed);
                                Err(format!("Failed to parse response: {}", e))
                            }
                        }
                    }
                    Err(e) => {
                        error!("Read error: {}", e);
                        Err(format!("Read error: {}", e))
                    }
                }
            }
            None => Err("Not connected to pool".to_string()),
        }
    }

    /// Get next request ID
    async fn get_next_request_id(&self) -> u64 {
        let mut id = self.request_id.lock().await;
        let current = *id;
        *id = current.wrapping_add(1);
        current
    }

    /// Check if connected
    pub async fn is_connected(&self) -> bool {
        let state = self.state.lock().await;
        *state != ConnectionState::Disconnected
    }

    /// Check if authorized
    pub async fn is_authorized(&self) -> bool {
        let state = self.state.lock().await;
        *state == ConnectionState::Authorized
    }

    /// Reconnect to pool
    pub async fn reconnect(&self) -> Result<(), String> {
        // Close existing connection
        let mut conn = self.connection.lock().await;
        *conn = None;
        drop(conn);

        // Reset state
        let mut state = self.state.lock().await;
        *state = ConnectionState::Disconnected;
        drop(state);

        // Reconnect
        self.connect().await
    }
}
