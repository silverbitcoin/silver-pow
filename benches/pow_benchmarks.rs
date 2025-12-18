//! Benchmarks for PoW mining

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use silver_pow::{
    DifficultyCalculator, Miner, MinerConfig, MiningPool, PoolConfig, PoWConfig,
    RewardCalculator, WorkPackage,
};

fn benchmark_work_package_creation(c: &mut Criterion) {
    c.bench_function("create_work_package", |b| {
        b.iter(|| {
            let _ = WorkPackage::new(
                0,
                100,
                vec![1u8; 32],
                vec![2u8; 32],
                vec![3u8; 32],
                1000,
                1_000_000,
            );
        });
    });
}

fn benchmark_difficulty_calculation(c: &mut Criterion) {
    c.bench_function("calculate_difficulty", |b| {
        b.iter(|| {
            let config = PoWConfig::default();
            let calc = DifficultyCalculator::new(config);

            let block_times = vec![30_000; 2016];
            let _ = calc.calculate_difficulty(1_000_000, &block_times, 2016);
        });
    });
}

fn benchmark_reward_calculation(c: &mut Criterion) {
    c.bench_function("calculate_block_reward", |b| {
        b.iter(|| {
            let config = PoWConfig::default();
            let calc = RewardCalculator::new(config);

            let _ = calc.calculate_total_reward(0, 1_000_000);
        });
    });
}

fn benchmark_miner_creation(c: &mut Criterion) {
    c.bench_function("create_miner", |b| {
        b.iter(|| {
            let config = MinerConfig::new(vec![1, 2, 3], vec![4, 5, 6], 4);
            let _miner = Miner::new(config);
        });
    });
}

fn benchmark_mining_pool_creation(c: &mut Criterion) {
    c.bench_function("create_mining_pool", |b| {
        b.iter(|| {
            let config = PoolConfig::new(vec![1, 2, 3], vec![4, 5, 6]);
            let _pool = MiningPool::new(config);
        });
    });
}

fn benchmark_pool_miner_registration(c: &mut Criterion) {
    c.bench_function("register_100_miners", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                let config = PoolConfig::new(vec![1, 2, 3], vec![4, 5, 6]);
                let pool = MiningPool::new(config);

                for i in 0..100 {
                    let miner_id = format!("miner_{}", i).into_bytes();
                    let miner_addr = format!("addr_{}", i).into_bytes();
                    let _ = pool.register_miner(miner_id, miner_addr).await;
                }
            });
    });
}

criterion_group!(
    benches,
    benchmark_work_package_creation,
    benchmark_difficulty_calculation,
    benchmark_reward_calculation,
    benchmark_miner_creation,
    benchmark_mining_pool_creation,
    benchmark_pool_miner_registration,
);

criterion_main!(benches);
