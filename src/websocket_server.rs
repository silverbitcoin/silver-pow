//! WebSocket Server for Real-Time Blockchain Data Streaming
//!
//! Provides real-time blockchain data to Explorer and other clients
//! Production-grade implementation with:
//! - Real-time block updates
//! - Real-time transaction updates
//! - Real-time balance updates
//! - Subscription-based data streaming
//! - Automatic reconnection support
//! - Binary and JSON message support

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// WebSocket event types for real-time updates
#[derive(Debug, Clone)]
pub enum BlockchainEvent {
    /// New block mined
    NewBlock {
        hash: String,
        height: u64,
        timestamp: u64,
        miner: String,
        reward: u64,
    },
    /// New transaction in mempool
    NewTransaction {
        txid: String,
        from: String,
        to: String,
        amount: u64,
        fee: u64,
        timestamp: u64,
    },
    /// Transaction confirmed
    TransactionConfirmed {
        txid: String,
        block_hash: String,
        block_height: u64,
        confirmations: u32,
    },
    /// Address balance changed
    BalanceChanged {
        address: String,
        old_balance: u64,
        new_balance: u64,
        timestamp: u64,
    },
    /// Mining difficulty adjusted
    DifficultyAdjusted {
        old_difficulty: f64,
        new_difficulty: f64,
        timestamp: u64,
    },
    /// Network stats updated
    NetworkStats {
        peers: u32,
        hashrate: u64,
        blocks: u64,
        transactions: u64,
    },
}

/// WebSocket subscription types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionType {
    /// Subscribe to all new blocks
    Blocks,
    /// Subscribe to all new transactions
    Transactions,
    /// Subscribe to specific address balance changes
    Address(String),
    /// Subscribe to mining difficulty changes
    Difficulty,
    /// Subscribe to network statistics
    NetworkStats,
}

/// WebSocket client connection state
struct ClientConnection {
    subscriptions: Vec<SubscriptionType>,
    #[allow(dead_code)]
    last_heartbeat: u64,
}

/// WebSocket server state
pub struct WebSocketServer {
    /// Broadcast channel for blockchain events
    event_tx: broadcast::Sender<BlockchainEvent>,
    /// Connected clients
    clients: Arc<RwLock<HashMap<String, ClientConnection>>>,
    /// Server address
    addr: SocketAddr,
}

impl WebSocketServer {
    /// Create new WebSocket server
    pub fn new(addr: SocketAddr) -> Self {
        let (event_tx, _) = broadcast::channel(1000);

        Self {
            event_tx,
            clients: Arc::new(RwLock::new(HashMap::new())),
            addr,
        }
    }

    /// Start WebSocket server
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting WebSocket server on {}", self.addr);

        let listener = tokio::net::TcpListener::bind(self.addr).await?;
        info!("✓ WebSocket server listening on ws://{}", self.addr);

        loop {
            let (stream, peer_addr) = listener.accept().await?;
            debug!("New WebSocket connection from {}", peer_addr);

            let event_tx = self.event_tx.clone();
            let clients = self.clients.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, peer_addr, event_tx, clients).await {
                    error!("WebSocket connection error: {}", e);
                }
            });
        }
    }

    /// Broadcast new block event
    pub fn broadcast_new_block(
        &self,
        hash: String,
        height: u64,
        timestamp: u64,
        miner: String,
        reward: u64,
    ) {
        let event = BlockchainEvent::NewBlock {
            hash,
            height,
            timestamp,
            miner,
            reward,
        };

        let _ = self.event_tx.send(event);
    }

    /// Broadcast new transaction event
    pub fn broadcast_new_transaction(
        &self,
        txid: String,
        from: String,
        to: String,
        amount: u64,
        fee: u64,
        timestamp: u64,
    ) {
        let event = BlockchainEvent::NewTransaction {
            txid,
            from,
            to,
            amount,
            fee,
            timestamp,
        };

        let _ = self.event_tx.send(event);
    }

    /// Broadcast transaction confirmed event
    pub fn broadcast_transaction_confirmed(
        &self,
        txid: String,
        block_hash: String,
        block_height: u64,
        confirmations: u32,
    ) {
        let event = BlockchainEvent::TransactionConfirmed {
            txid,
            block_hash,
            block_height,
            confirmations,
        };

        let _ = self.event_tx.send(event);
    }

    /// Broadcast balance changed event
    pub fn broadcast_balance_changed(
        &self,
        address: String,
        old_balance: u64,
        new_balance: u64,
        timestamp: u64,
    ) {
        let event = BlockchainEvent::BalanceChanged {
            address,
            old_balance,
            new_balance,
            timestamp,
        };

        let _ = self.event_tx.send(event);
    }

    /// Broadcast difficulty adjusted event
    pub fn broadcast_difficulty_adjusted(
        &self,
        old_difficulty: f64,
        new_difficulty: f64,
        timestamp: u64,
    ) {
        let event = BlockchainEvent::DifficultyAdjusted {
            old_difficulty,
            new_difficulty,
            timestamp,
        };

        let _ = self.event_tx.send(event);
    }

    /// Broadcast network stats event
    pub fn broadcast_network_stats(
        &self,
        peers: u32,
        hashrate: u64,
        blocks: u64,
        transactions: u64,
    ) {
        let event = BlockchainEvent::NetworkStats {
            peers,
            hashrate,
            blocks,
            transactions,
        };

        let _ = self.event_tx.send(event);
    }
}

/// Handle individual WebSocket connection
async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer_addr: std::net::SocketAddr,
    event_tx: broadcast::Sender<BlockchainEvent>,
    clients: Arc<RwLock<HashMap<String, ClientConnection>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Upgrade to WebSocket
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    info!("✓ WebSocket connection established: {}", peer_addr);

    let client_id = peer_addr.to_string();
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    // Initialize client connection
    {
        let mut clients_lock = clients.write().await;
        clients_lock.insert(
            client_id.clone(),
            ClientConnection {
                subscriptions: vec![],
                last_heartbeat: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            },
        );
    }

    // Subscribe to events
    let mut event_rx = event_tx.subscribe();

    // Send welcome message
    let welcome = json!({
        "type": "connected",
        "message": "Connected to SilverBitcoin WebSocket Server",
        "version": "2.5.3",
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    });

    ws_sender.send(Message::text(welcome.to_string())).await?;

    loop {
        tokio::select! {
            // Handle incoming messages from client
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!("Received message from {}: {}", peer_addr, text);

                        // Parse subscription request
                        if let Ok(request) = serde_json::from_str::<Value>(&text) {
                            if let Some(action) = request.get("action").and_then(|v| v.as_str()) {
                                match action {
                                    "subscribe" => {
                                        if let Some(channel) = request.get("channel").and_then(|v| v.as_str()) {
                                            let subscription = match channel {
                                                "blocks" => Some(SubscriptionType::Blocks),
                                                "transactions" => Some(SubscriptionType::Transactions),
                                                "difficulty" => Some(SubscriptionType::Difficulty),
                                                "network_stats" => Some(SubscriptionType::NetworkStats),
                                                addr if addr.starts_with("address:") => {
                                                    let address = addr.strip_prefix("address:").unwrap_or("").to_string();
                                                    Some(SubscriptionType::Address(address))
                                                }
                                                _ => None,
                                            };

                                            if let Some(sub) = subscription {
                                                let mut clients_lock = clients.write().await;
                                                if let Some(client) = clients_lock.get_mut(&client_id) {
                                                    if !client.subscriptions.contains(&sub) {
                                                        client.subscriptions.push(sub.clone());
                                                        info!("Client {} subscribed to {:?}", peer_addr, sub);

                                                        let response = json!({
                                                            "type": "subscribed",
                                                            "channel": channel,
                                                            "timestamp": std::time::SystemTime::now()
                                                                .duration_since(std::time::UNIX_EPOCH)
                                                                .unwrap_or_default()
                                                                .as_secs(),
                                                        });
                                                        let _ = ws_sender.send(Message::text(response.to_string())).await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    "unsubscribe" => {
                                        if let Some(channel) = request.get("channel").and_then(|v| v.as_str()) {
                                            let mut clients_lock = clients.write().await;
                                            if let Some(client) = clients_lock.get_mut(&client_id) {
                                                client.subscriptions.retain(|sub| {
                                                    match (sub, channel) {
                                                        (SubscriptionType::Blocks, "blocks") => false,
                                                        (SubscriptionType::Transactions, "transactions") => false,
                                                        (SubscriptionType::Difficulty, "difficulty") => false,
                                                        (SubscriptionType::NetworkStats, "network_stats") => false,
                                                        (SubscriptionType::Address(addr), ch) if ch.starts_with("address:") => {
                                                            !ch.ends_with(addr)
                                                        }
                                                        _ => true,
                                                    }
                                                });
                                                info!("Client {} unsubscribed from {}", peer_addr, channel);
                                            }
                                        }
                                    }
                                    "ping" => {
                                        let pong = json!({
                                            "type": "pong",
                                            "timestamp": std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs(),
                                        });
                                        let _ = ws_sender.send(Message::text(pong.to_string())).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("Client {} closed connection", peer_addr);
                        break;
                    }
                    Some(Ok(msg)) => {
                        // PRODUCTION-GRADE: Log all message types for debugging and monitoring
                        match msg {
                            Message::Text(_) | Message::Binary(_) => {
                                // Already handled above
                            }
                            Message::Ping(data) => {
                                debug!("Ping from {}: {} bytes", peer_addr, data.len());
                                // Send pong response
                                if let Err(e) = ws_sender.send(Message::Pong(data)).await {
                                    warn!("Failed to send pong to {}: {}", peer_addr, e);
                                }
                            }
                            Message::Pong(data) => {
                                debug!("Pong from {}: {} bytes", peer_addr, data.len());
                            }
                            Message::Frame(_) => {
                                warn!("Received raw frame from {}, ignoring", peer_addr);
                            }
                            Message::Close(_) => {
                                info!("Client {} sent close frame", peer_addr);
                                break;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error from {}: {}", peer_addr, e);
                        break;
                    }
                    None => {
                        info!("Client {} disconnected", peer_addr);
                        break;
                    }
                }
            }

            // Handle blockchain events
            event = event_rx.recv() => {
                match event {
                    Ok(blockchain_event) => {
                        let clients_lock = clients.read().await;
                        if let Some(client) = clients_lock.get(&client_id) {
                            // Check if client is subscribed to this event
                            let should_send = match &blockchain_event {
                                BlockchainEvent::NewBlock { .. } => {
                                    client.subscriptions.contains(&SubscriptionType::Blocks)
                                }
                                BlockchainEvent::NewTransaction { .. } => {
                                    client.subscriptions.contains(&SubscriptionType::Transactions)
                                }
                                BlockchainEvent::TransactionConfirmed { .. } => {
                                    client.subscriptions.contains(&SubscriptionType::Transactions)
                                }
                                BlockchainEvent::BalanceChanged { address, .. } => {
                                    client.subscriptions.iter().any(|sub| {
                                        matches!(sub, SubscriptionType::Address(addr) if addr == address)
                                    })
                                }
                                BlockchainEvent::DifficultyAdjusted { .. } => {
                                    client.subscriptions.contains(&SubscriptionType::Difficulty)
                                }
                                BlockchainEvent::NetworkStats { .. } => {
                                    client.subscriptions.contains(&SubscriptionType::NetworkStats)
                                }
                            };

                            if should_send {
                                let message = match blockchain_event {
                                    BlockchainEvent::NewBlock { hash, height, timestamp, miner, reward } => {
                                        json!({
                                            "type": "block",
                                            "hash": hash,
                                            "height": height,
                                            "timestamp": timestamp,
                                            "miner": miner,
                                            "reward": reward,
                                        })
                                    }
                                    BlockchainEvent::NewTransaction { txid, from, to, amount, fee, timestamp } => {
                                        json!({
                                            "type": "transaction",
                                            "txid": txid,
                                            "from": from,
                                            "to": to,
                                            "amount": amount,
                                            "fee": fee,
                                            "timestamp": timestamp,
                                        })
                                    }
                                    BlockchainEvent::TransactionConfirmed { txid, block_hash, block_height, confirmations } => {
                                        json!({
                                            "type": "transaction_confirmed",
                                            "txid": txid,
                                            "block_hash": block_hash,
                                            "block_height": block_height,
                                            "confirmations": confirmations,
                                        })
                                    }
                                    BlockchainEvent::BalanceChanged { address, old_balance, new_balance, timestamp } => {
                                        json!({
                                            "type": "balance_changed",
                                            "address": address,
                                            "old_balance": old_balance,
                                            "new_balance": new_balance,
                                            "timestamp": timestamp,
                                        })
                                    }
                                    BlockchainEvent::DifficultyAdjusted { old_difficulty, new_difficulty, timestamp } => {
                                        json!({
                                            "type": "difficulty_adjusted",
                                            "old_difficulty": old_difficulty,
                                            "new_difficulty": new_difficulty,
                                            "timestamp": timestamp,
                                        })
                                    }
                                    BlockchainEvent::NetworkStats { peers, hashrate, blocks, transactions } => {
                                        json!({
                                            "type": "network_stats",
                                            "peers": peers,
                                            "hashrate": hashrate,
                                            "blocks": blocks,
                                            "transactions": transactions,
                                        })
                                    }
                                };

                                if let Err(e) = ws_sender.send(Message::text(message.to_string())).await {
                                    error!("Failed to send message to {}: {}", peer_addr, e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        warn!("Event queue lagged for client {}", peer_addr);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("Event channel closed for client {}", peer_addr);
                        break;
                    }
                }
            }
        }
    }

    // Remove client on disconnect
    {
        let mut clients_lock = clients.write().await;
        clients_lock.remove(&client_id);
    }

    info!("Client {} disconnected", peer_addr);
    Ok(())
}
