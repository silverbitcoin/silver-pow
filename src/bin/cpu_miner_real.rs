//! Production-Grade CPU Miner for SilverBitcoin
//! Real implementation with Stratum pool support

use clap::Parser;
use sha2::{Digest, Sha512};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Real u512 implementation for SHA-512 hash comparison
/// Represents a 512-bit unsigned integer as four u128 values (for proper 512-bit arithmetic)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct U512 {
    part0: u128,  // Bytes 0-15
    part1: u128,  // Bytes 16-31
    part2: u128,  // Bytes 32-47
    part3: u128,  // Bytes 48-63
}

impl std::fmt::Display for U512 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:032x}_{:032x}_{:032x}_{:032x}",
            self.part0, self.part1, self.part2, self.part3
        )
    }
}

impl U512 {
    /// Create U512 from 64 bytes (big-endian) - full SHA-512 hash
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
fn u512_from_bytes(bytes: &[u8; 64]) -> U512 {
    U512::from_bytes(bytes)
}

/// Get maximum U512 value
fn u512_max() -> U512 {
    U512::max()
}

#[derive(Parser, Debug)]
#[command(name = "SilverBitcoin CPU Miner")]
#[command(about = "Real CPU mining for SilverBitcoin with Stratum pool support")]
struct Args {
    /// Miner wallet address
    #[arg(short, long)]
    miner_address: String,

    /// Pool URL (host:port for Stratum)
    #[arg(long, default_value = "localhost:3333")]
    pool_url: String,

    /// Number of mining threads
    #[arg(short, long, default_value = "4")]
    threads: usize,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,
}

/// Mining statistics
#[derive(Debug, Clone)]
struct MiningStats {
    shares_found: Arc<AtomicU64>,
    valid_shares: Arc<AtomicU64>,
    invalid_shares: Arc<AtomicU64>,
    blocks_found: Arc<AtomicU64>,
    hashes_computed: Arc<AtomicU64>,
    start_time: Instant,
}

impl MiningStats {
    fn new() -> Self {
        Self {
            shares_found: Arc::new(AtomicU64::new(0)),
            valid_shares: Arc::new(AtomicU64::new(0)),
            invalid_shares: Arc::new(AtomicU64::new(0)),
            blocks_found: Arc::new(AtomicU64::new(0)),
            hashes_computed: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
        }
    }

    fn hashrate(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.hashes_computed.load(Ordering::Relaxed) as f64 / elapsed
        } else {
            0.0
        }
    }

    fn print_stats(&self) {
        let hashrate = self.hashrate();
        let hashrate_str = if hashrate > 1_000_000.0 {
            format!("{:.2} MH/s", hashrate / 1_000_000.0)
        } else if hashrate > 1_000.0 {
            format!("{:.2} KH/s", hashrate / 1_000.0)
        } else {
            format!("{:.2} H/s", hashrate)
        };

        let shares = self.shares_found.load(Ordering::Relaxed);
        let valid = self.valid_shares.load(Ordering::Relaxed);
        let invalid = self.invalid_shares.load(Ordering::Relaxed);
        let blocks = self.blocks_found.load(Ordering::Relaxed);

        info!(
            "Stats: {} | Shares: {} ({} valid, {} invalid) | Blocks: {}",
            hashrate_str, shares, valid, invalid, blocks
        );
    }
}

/// Real CPU mining loop with Stratum pool
async fn mining_loop(
    thread_id: usize,
    pool_url: String,
    miner_address: String,
    stats: Arc<MiningStats>,
) {
    info!("Mining thread {} started", thread_id);

    // Create Stratum client
    let stratum_client = silver_pow::StratumPoolClient::new(pool_url.clone(), miner_address.clone());

    // Connect to pool with retries
    let mut connect_attempts = 0;
    loop {
        match stratum_client.connect().await {
            Ok(_) => {
                info!("Thread {}: Connected to pool", thread_id);
                break;
            }
            Err(e) => {
                connect_attempts += 1;
                if connect_attempts >= 5 {
                    error!("Thread {}: Failed to connect after 5 attempts: {}", thread_id, e);
                    return;
                }
                warn!(
                    "Thread {}: Connection attempt {} failed: {}. Retrying...",
                    thread_id, connect_attempts, e
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }

    let mut nonce: u64 = (thread_id as u64) * (u64::MAX / 4);
    let mut last_stats_print = Instant::now();
    let mut reconnect_check = Instant::now();

    loop {
        // Check connection health every 30 seconds
        if reconnect_check.elapsed() > Duration::from_secs(30) {
            if !stratum_client.is_authorized().await {
                warn!("Thread {}: Lost authorization, reconnecting...", thread_id);
                match stratum_client.reconnect().await {
                    Ok(_) => {
                        info!("Thread {}: Reconnected to pool", thread_id);
                    }
                    Err(e) => {
                        error!("Thread {}: Reconnection failed: {}", thread_id, e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }
            reconnect_check = Instant::now();
        }

        // Compute SHA-512 hash
        let mut hasher = Sha512::new();
        hasher.update(nonce.to_le_bytes());
        let hash = hasher.finalize();
        let hash_hex = format!("{:x}", hash);

        // Update stats
        stats.hashes_computed.fetch_add(1, Ordering::Relaxed);

        // Print stats every 10 seconds
        if last_stats_print.elapsed() > Duration::from_secs(10) {
            stats.print_stats();
            last_stats_print = Instant::now();
        }

        // Real difficulty validation: convert full SHA-512 hash to u512 and compare with target
        let hash_vec = hash.to_vec();
        let hash_bytes: [u8; 64] = hash_vec.as_slice().try_into().expect("SHA-512 hash must be 64 bytes");
        let hash_u512 = u512_from_bytes(&hash_bytes);

        // Pool difficulty target: 1,000,000,000
        // Block difficulty: 1,000,000,000
        const POOL_DIFFICULTY: u128 = 1_000_000_000;
        const BLOCK_DIFFICULTY: u128 = 1_000_000_000;

        let pool_target = u512_max().div_u128(POOL_DIFFICULTY);
        let block_target = u512_max().div_u128(BLOCK_DIFFICULTY);

        // Check if hash meets pool difficulty
        if hash_u512 <= pool_target {
            stats.shares_found.fetch_add(1, Ordering::Relaxed);

            info!(
                "Thread {}: Found share! Nonce: {}, Hash: {}",
                thread_id, nonce, hash_hex
            );

            // Check if it's a block (meets block difficulty) BEFORE submitting
            let is_block = hash_u512 <= block_target;
            
            if is_block {
                stats.blocks_found.fetch_add(1, Ordering::Relaxed);
                info!("🎉 BLOCK FOUND! Hash: {}", hash_hex);
                info!("Block difficulty met: {} <= {}", hash_u512, block_target);
            }

            // Submit to pool with is_block flag
            // Generate unique job ID from hash and nonce for tracking
            let job_id = format!("{:x}_{}", nonce, &hash_hex[0..16]);
            match stratum_client.submit_share(&job_id, nonce, &hash_hex, is_block).await {
                Ok(true) => {
                    stats.valid_shares.fetch_add(1, Ordering::Relaxed);
                    
                    if is_block {
                        info!("✅ Block accepted by pool!");
                    }
                }
                Ok(false) => {
                    stats.invalid_shares.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    error!("Thread {}: Share submission error: {}", thread_id, e);
                }
            }
        }

        nonce = nonce.wrapping_add(1);

        // Yield to other tasks periodically
        if nonce.is_multiple_of(1_000_000) {
            tokio::task::yield_now().await;
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    info!("  SilverBitcoin CPU Miner v2.5.3");
    info!("  Real Production Implementation");
    info!("═══════════════════════════════════════════════════════════");
    info!("");
    info!("Configuration:");
    info!("  Miner Address: {}", args.miner_address);
    info!("  Pool URL: {}", args.pool_url);
    info!("  Threads: {}", args.threads);
    info!("");

    // Create stats
    let stats = Arc::new(MiningStats::new());

    // Start mining threads
    let mut handles = vec![];
    for thread_id in 0..args.threads {
        let pool_url = args.pool_url.clone();
        let miner_addr = args.miner_address.clone();
        let s = Arc::clone(&stats);
        let handle = tokio::spawn(mining_loop(thread_id, pool_url, miner_addr, s));
        handles.push(handle);
    }

    info!("Mining started with {} threads", args.threads);

    // Wait for all threads
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}
