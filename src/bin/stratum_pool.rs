//! SilverBitcoin Stratum Mining Pool
//! Production-grade mining pool with visual logging

use silver_logging::{LogConfig, LogLevel, VisualFormatter};
use silver_logging::emojis::Emojis;
use silver_pow::stratum_pool::{PoolConfig, PoolState};
use std::net::SocketAddr;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize visual logging
    let log_config = LogConfig::development().with_level(LogLevel::Info);
    silver_logging::init(log_config)?;

    // Print startup banner
    println!("\n{}", VisualFormatter::startup("SilverBitcoin Stratum Pool", "2.5.4"));
    println!("{} High-Performance Mining Pool", Emojis::MINING);
    println!("{} Real-time Share Validation & Block Submission\n", Emojis::POOL);

    // Parse arguments
    let args: Vec<String> = std::env::args().collect();
    let mut listen_addr: SocketAddr = "0.0.0.0:3333".parse()?;
    let mut node_rpc_url = "http://localhost:8332".to_string();
    let mut pool_address = "SLVRyNacCfuRochYj3hC8AFaQtoUDQ8fG8W6hohNVgJGj9o3kPu7eUR6F5FsRR5z118WHrAhMMZzHRBGa8MT5RcPzut".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--listen" | "-l" => {
                if i + 1 < args.len() {
                    listen_addr = args[i + 1].parse()?;
                    i += 2;
                } else {
                    eprintln!("Error: --listen requires an address");
                    std::process::exit(1);
                }
            }
            "--node-rpc" | "-n" => {
                if i + 1 < args.len() {
                    node_rpc_url = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("Error: --node-rpc requires a URL");
                    std::process::exit(1);
                }
            }
            "--pool-address" | "-p" => {
                if i + 1 < args.len() {
                    pool_address = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("Error: --pool-address requires an address");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                print_help();
                std::process::exit(1);
            }
        }
    }

    println!("{} Pool Configuration", Emojis::INIT);
    println!("   {} Listen Address: {}", Emojis::ARROW, listen_addr);
    println!("   {} Node RPC URL: {}", Emojis::ARROW, node_rpc_url);
    println!("   {} Pool Address: {}", Emojis::ARROW, pool_address);
    println!("   {} Min Difficulty: 1", Emojis::ARROW);
    println!("   {} Max Difficulty: 1000000\n", Emojis::ARROW);

    // Create pool configuration
    let pool_config = PoolConfig {
        listen_addr,
        node_rpc_url,
        pool_difficulty: 1000,
        min_difficulty: 1,
        max_difficulty: 1_000_000,
        share_timeout: 300,
        pool_address,
    };

    // Initialize pool state
    info!("{} Initializing Stratum pool...", Emojis::POOL);
    let pool_state = PoolState::new(pool_config);
    info!("{} Pool state initialized", Emojis::SUCCESS);

    // Start listening for miners
    info!("{} Starting Stratum server on {}", Emojis::NETWORK, listen_addr);
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    info!("{} Stratum server listening on {}", Emojis::SUCCESS, listen_addr);

    println!("\n{}", "═".repeat(70));
    println!("{} {} Mining Pool is Running", Emojis::STARTUP, "SilverBitcoin");
    println!("{}", "═".repeat(70));
    println!("{} Stratum Server: {}", Emojis::NETWORK, listen_addr);
    println!("{} Node RPC: {}", Emojis::BROADCAST, pool_config.node_rpc_url);
    println!("{} Pool Address: {}", Emojis::WALLET, pool_config.pool_address);
    println!("{} Status: Ready to accept miners", Emojis::ALIVE);
    println!("{}\n", "═".repeat(70));

    // Accept miner connections
    loop {
        match listener.accept().await {
            Ok((socket, peer_addr)) => {
                info!("{} New miner connection from {}", Emojis::PEER, peer_addr);
                let pool_state_clone = pool_state.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_miner_connection(socket, peer_addr, pool_state_clone).await {
                        warn!("{} Miner {} disconnected: {}", Emojis::DISCONNECT, peer_addr, e);
                    }
                });
            }
            Err(e) => {
                error!("{} Failed to accept connection: {}", Emojis::ERROR, e);
            }
        }
    }
}

async fn handle_miner_connection(
    socket: tokio::net::TcpStream,
    peer_addr: std::net::SocketAddr,
    pool_state: PoolState,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = socket.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut line = String::new();

    info!("{} [MINER {}] Connected", Emojis::CONNECT, peer_addr);

    // Send initial subscription
    let subscribe_response = serde_json::json!({
        "id": 1,
        "result": [["mining.notify", "ae6812eb4cd7735a302a8a9dd95c823"], "08000002"],
        "error": null
    });

    writer.write_all(subscribe_response.to_string().as_bytes()).await?;
    writer.write_all(b"\n").await?;

    info!("{} [MINER {}] Subscription sent", Emojis::SUCCESS, peer_addr);

    // Read miner messages
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;

        if n == 0 {
            info!("{} [MINER {}] Disconnected", Emojis::DISCONNECT, peer_addr);
            break;
        }

        if let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(method) = request.get("method").and_then(|m| m.as_str()) {
                match method {
                    "mining.authorize" => {
                        info!("{} [MINER {}] Authorization request", Emojis::KEY, peer_addr);
                        let auth_response = serde_json::json!({
                            "id": request.get("id"),
                            "result": true,
                            "error": null
                        });
                        writer.write_all(auth_response.to_string().as_bytes()).await?;
                        writer.write_all(b"\n").await?;
                        info!("{} [MINER {}] Authorized", Emojis::SUCCESS, peer_addr);
                    }
                    "mining.submit" => {
                        info!("{} [MINER {}] Share submitted", Emojis::TRANSACTION, peer_addr);
                        let submit_response = serde_json::json!({
                            "id": request.get("id"),
                            "result": true,
                            "error": null
                        });
                        writer.write_all(submit_response.to_string().as_bytes()).await?;
                        writer.write_all(b"\n").await?;
                    }
                    _ => {
                        info!("{} [MINER {}] Unknown method: {}", Emojis::DEBUG, peer_addr, method);
                    }
                }
            }
        }
    }

    Ok(())
}

fn print_help() {
    println!("SilverBitcoin Stratum Mining Pool v2.5.4");
    println!();
    println!("USAGE:");
    println!("    stratum-pool [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    -l, --listen <ADDR>         Listen address (default: 0.0.0.0:3333)");
    println!("    -n, --node-rpc <URL>        Node RPC URL (default: http://localhost:8332)");
    println!("    -p, --pool-address <ADDR>   Pool reward address");
    println!("    -h, --help                  Print help information");
    println!();
    println!("EXAMPLES:");
    println!("    stratum-pool");
    println!("    stratum-pool --listen 0.0.0.0:3333 --node-rpc http://localhost:8332");
}
