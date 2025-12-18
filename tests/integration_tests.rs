//! Integration tests for the complete PoW mining system

use silver_pow::{
    DifficultyCalculator, Miner, MinerConfig, MiningPool, PoolConfig, PoWConfig, RewardCalculator,
    WorkPackage,
};

#[tokio::test]
async fn test_complete_mining_workflow() {
    // Create configuration
    let pow_config = PoWConfig::default();
    let pool_config = PoolConfig::new().with_fee(1);

    // Create mining pool
    let pool = MiningPool::new(pool_config);

    // Register miners
    for i in 0..5 {
        let miner_id = format!("miner_{}", i).into_bytes();
        let miner_addr = format!("addr_{}", i).into_bytes();
        assert!(pool.register_miner(miner_id, miner_addr).await.is_ok());
    }

    assert_eq!(pool.get_miner_count().await, 5);

    // Create work package
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

    // Distribute work
    assert!(pool.distribute_work(work).await.is_ok());

    // Get pool stats
    let stats = pool.get_stats().await;
    assert_eq!(stats.connected_miners, 5);
}

#[tokio::test]
async fn test_miner_registration_and_shares() {
    let pool_config = PoolConfig::new();
    let pool = MiningPool::new(pool_config);

    let miner_id = vec![1, 2, 3];
    let miner_addr = vec![4, 5, 6];

    // Register miner
    assert!(pool.register_miner(miner_id.clone(), miner_addr).await.is_ok());

    // Get miner account
    let account = pool.get_miner_account(&miner_id).await;
    assert!(account.is_some());
    let account = account.unwrap();
    assert_eq!(account.total_shares, 0);

    // Submit shares
    for i in 0..10 {
        let share = silver_pow::MinerShare::new(
            miner_id.clone(),
            vec![i as u8; 64],
            i as u64 * 1000,
            0,
            vec![0u8; 64],
            10_000,
            false,
            0,
            100,
        )
        .unwrap();

        assert!(pool.submit_share(share).await.is_ok());
    }

    // Check miner shares
    let shares = pool.get_miner_shares(&miner_id).await;
    assert_eq!(shares.len(), 10);

    // Check pool stats
    let stats = pool.get_stats().await;
    assert_eq!(stats.shares_accepted, 10);
}

#[tokio::test]
async fn test_difficulty_adjustment() {
    let config = PoWConfig::default();
    let calc = DifficultyCalculator::new(config);

    // Test faster blocks (should increase difficulty)
    let block_times = vec![15_000; 2016]; // 15 seconds each instead of 30
    let new_diff = calc
        .adjust_difficulty_kadena_style(1_000_000, 2016, 15_000 * 2016)
        .unwrap();
    assert!(new_diff > 1_000_000);

    // Test slower blocks (should decrease difficulty)
    let new_diff = calc
        .adjust_difficulty_kadena_style(1_000_000, 2016, 60_000 * 2016)
        .unwrap();
    assert!(new_diff < 1_000_000);
}

#[tokio::test]
async fn test_block_rewards_and_halving() {
    let config = PoWConfig::default();
    let calc = RewardCalculator::new(config);

    // Test base reward
    let reward_at_0 = calc.calculate_base_reward(0);
    assert_eq!(reward_at_0, config.base_block_reward);

    // Test halving
    let reward_at_halving = calc.calculate_base_reward(config.halving_interval);
    assert_eq!(reward_at_halving, config.base_block_reward / 2);

    // Test multiple halvings
    let reward_at_2x = calc.calculate_base_reward(config.halving_interval * 2);
    assert_eq!(reward_at_2x, config.base_block_reward / 4);

    // Test miner reward (100% to miners)
    let miner_reward = calc.calculate_miner_reward(0, 1_000_000).unwrap();
    assert_eq!(miner_reward, config.base_block_reward + 1_000_000);
}

#[tokio::test]
async fn test_work_package_and_proof() {
    // Create work package
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

    assert_eq!(work.chain_id, 0);
    assert_eq!(work.block_height, 100);
    assert_eq!(work.target.len(), 64);

    // Create work proof
    let proof = silver_pow::WorkProof::new(
        work.work_id.clone(),
        0,
        100,
        12345,
        0,
        vec![1u8; 32],
        vec![0u8; 64],
        1000,
        vec![3u8; 20],
    )
    .unwrap();

    // Verify proof
    assert!(proof.verify(&work.target).is_ok());
}

#[tokio::test]
async fn test_miner_creation_and_stats() {
    let config = MinerConfig::new(4);
    let miner = Miner::new(config);

    // Check initial stats
    let stats = miner.get_stats().await;
    assert_eq!(stats.total_hashes, 0);
    assert_eq!(stats.valid_proofs, 0);

    // Check uptime
    let uptime = miner.get_uptime();
    assert!(uptime >= 0);
}

#[tokio::test]
async fn test_pool_fee_calculation() {
    let config = PoolConfig::new().with_fee(2);
    let pool = MiningPool::new(config);

    let miner_id = vec![1, 2, 3];

    // Register miner
    pool.register_miner(miner_id.clone(), vec![4, 5, 6])
        .await
        .unwrap();

    // Submit share
    let share = silver_pow::MinerShare::new(
        miner_id.clone(),
        vec![1u8; 64],
        12345,
        0,
        vec![0u8; 64],
        10_000,
        false,
        0,
        100,
    )
    .unwrap();

    pool.submit_share(share).await.unwrap();

    // Calculate payout with 2% fee
    let total_reward = 50_000_000_000u128;
    let payout = pool.calculate_miner_payout(&miner_id, total_reward).await.unwrap();

    // With 2% fee, miner should get 98% of reward
    let expected = (total_reward * 98) / 100;
    assert_eq!(payout, expected);
}

#[tokio::test]
async fn test_multiple_miners_share_distribution() {
    let pool_config = PoolConfig::new().with_fee(0); // No fee for this test
    let pool = MiningPool::new(pool_config);

    // Register 3 miners
    let miners = vec![
        (vec![1u8], vec![10u8]),
        (vec![2u8], vec![20u8]),
        (vec![3u8], vec![30u8]),
    ];

    for (miner_id, miner_addr) in &miners {
        pool.register_miner(miner_id.clone(), miner_addr.clone())
            .await
            .unwrap();
    }

    // Submit shares with different difficulties
    let difficulties = vec![10_000, 20_000, 30_000];

    for (i, (miner_id, _)) in miners.iter().enumerate() {
        let share = silver_pow::MinerShare::new(
            miner_id.clone(),
            vec![i as u8; 64],
            i as u64 * 1000,
            0,
            vec![0u8; 64],
            difficulties[i],
            false,
            0,
            100,
        )
        .unwrap();

        pool.submit_share(share).await.unwrap();
    }

    // Calculate payouts
    let total_reward = 50_000_000_000u128;
    let total_difficulty = (10_000 + 20_000 + 30_000) as u128;

    for (i, (miner_id, _)) in miners.iter().enumerate() {
        let payout = pool.calculate_miner_payout(miner_id, total_reward).await.unwrap();
        let expected = (total_reward * difficulties[i] as u128) / total_difficulty;
        assert_eq!(payout, expected);
    }
}

#[tokio::test]
async fn test_sha512_mining_hash() {
    let data = b"test mining data";
    let hash = WorkPackage::calculate_sha512_hash(data);

    // SHA-512 produces 64 bytes
    assert_eq!(hash.len(), 64);

    // Verify deterministic
    let hash2 = WorkPackage::calculate_sha512_hash(data);
    assert_eq!(hash, hash2);

    // Verify different data produces different hash
    let hash3 = WorkPackage::calculate_sha512_hash(b"different data");
    assert_ne!(hash, hash3);
}

#[tokio::test]
async fn test_double_sha512_mining_hash() {
    let data = b"test mining data";
    let double_hash = WorkPackage::calculate_double_sha512_hash(data);
    let single_hash = WorkPackage::calculate_sha512_hash(data);

    // Double hash should be different from single hash
    assert_ne!(double_hash, single_hash);

    // Double hash should still be 64 bytes
    assert_eq!(double_hash.len(), 64);

    // Verify deterministic
    let double_hash2 = WorkPackage::calculate_double_sha512_hash(data);
    assert_eq!(double_hash, double_hash2);
}

#[tokio::test]
async fn test_pool_statistics() {
    let pool_config = PoolConfig::new();
    let pool = MiningPool::new(pool_config);

    // Register miners
    for i in 0..3 {
        let miner_id = format!("miner_{}", i).into_bytes();
        let miner_addr = format!("addr_{}", i).into_bytes();
        pool.register_miner(miner_id, miner_addr).await.unwrap();
    }

    // Get stats
    let stats = pool.get_stats().await;
    assert_eq!(stats.connected_miners, 3);
    assert_eq!(stats.shares_accepted, 0);
    assert_eq!(stats.blocks_found, 0);
    assert!(stats.uptime_seconds >= 0);
}

#[tokio::test]
async fn test_work_package_validation() {
    // Test invalid chain ID
    let result = WorkPackage::new(
        20,
        100,
        vec![1u8; 32],
        vec![2u8; 32],
        vec![3u8; 32],
        1000,
        1_000_000,
    );
    assert!(result.is_err());

    // Test invalid hash length
    let result = WorkPackage::new(
        0,
        100,
        vec![1u8; 31],
        vec![2u8; 32],
        vec![3u8; 32],
        1000,
        1_000_000,
    );
    assert!(result.is_err());

    // Test zero difficulty
    let result = WorkPackage::new(
        0,
        100,
        vec![1u8; 32],
        vec![2u8; 32],
        vec![3u8; 32],
        1000,
        0,
    );
    assert!(result.is_err());
}
