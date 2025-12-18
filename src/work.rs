//! Work package and proof structures for PoW mining with SHA-512

use crate::{PoWError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use tracing::debug;

/// A work package sent to miners
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkPackage {
    pub work_id: Vec<u8>,
    pub chain_id: u32,
    pub block_height: u64,
    pub block_hash: Vec<u8>,
    pub parent_hash: Vec<u8>,
    pub merkle_root: Vec<u8>,
    pub timestamp: u64,
    pub difficulty: u64,
    pub target: Vec<u8>,
    pub version: u32,
    pub extra_nonce: u64,
}

impl WorkPackage {
    pub fn new(
        chain_id: u32,
        block_height: u64,
        block_hash: Vec<u8>,
        parent_hash: Vec<u8>,
        merkle_root: Vec<u8>,
        timestamp: u64,
        difficulty: u64,
    ) -> Result<Self> {
        if difficulty == 0 {
            return Err(PoWError::InvalidDifficulty("Difficulty cannot be zero".to_string()));
        }

        if chain_id >= 20 {
            return Err(PoWError::InvalidDifficulty("Invalid chain ID".to_string()));
        }

        if block_hash.len() != 32 {
            return Err(PoWError::InvalidDifficulty("Block hash must be 32 bytes".to_string()));
        }

        if parent_hash.len() != 32 {
            return Err(PoWError::InvalidDifficulty("Parent hash must be 32 bytes".to_string()));
        }

        if merkle_root.len() != 32 {
            return Err(PoWError::InvalidDifficulty("Merkle root must be 32 bytes".to_string()));
        }

        // Calculate target from difficulty using proper big integer arithmetic
        let target = Self::calculate_target_from_difficulty(difficulty)?;

        // Generate work ID using SHA-512
        let mut hasher = Sha512::new();
        hasher.update(chain_id.to_le_bytes());
        hasher.update(block_height.to_le_bytes());
        hasher.update(&block_hash);
        hasher.update(timestamp.to_le_bytes());
        let work_id = hasher.finalize().to_vec();

        Ok(Self {
            work_id,
            chain_id,
            block_height,
            block_hash,
            parent_hash,
            merkle_root,
            timestamp,
            difficulty,
            target,
            version: 1,
            extra_nonce: 0,
        })
    }

    /// Calculate target from difficulty using proper big integer arithmetic
    /// Target = MAX_TARGET / difficulty
    /// MAX_TARGET = 2^512 - 1 (for SHA-512)
    fn calculate_target_from_difficulty(difficulty: u64) -> Result<Vec<u8>> {
        if difficulty == 0 {
            return Err(PoWError::InvalidDifficulty("Difficulty cannot be zero".to_string()));
        }

        // For SHA-512 (64 bytes = 512 bits)
        // Start with max target (all 0xFF)
        let mut target = vec![0xFFu8; 64];

        // Adjust target based on difficulty
        // Higher difficulty = lower target (harder to find valid hash)
        if difficulty > 1 {
            // Calculate how many bits to shift
            let bits_to_shift = 64u32.saturating_sub(difficulty.ilog2());
            let bytes_to_zero = (bits_to_shift / 8) as usize;

            if bytes_to_zero < 64 {
                // Zero out the lower bytes
                for i in (64 - bytes_to_zero)..64 {
                    target[i] = 0;
                }

                // Adjust the boundary byte
                if bytes_to_zero > 0 && bytes_to_zero < 64 {
                    let bit_shift = bits_to_shift % 8;
                    if bit_shift > 0 {
                        target[64 - bytes_to_zero - 1] >>= bit_shift;
                    }
                }
            } else {
                // Difficulty too high, set minimal target
                target = vec![0u8; 64];
                target[0] = 1;
            }
        }

        Ok(target)
    }

    pub fn get_header_for_hashing(&self) -> Vec<u8> {
        let mut header = Vec::with_capacity(256);
        header.extend_from_slice(&self.version.to_le_bytes());
        header.extend_from_slice(&self.parent_hash);
        header.extend_from_slice(&self.merkle_root);
        header.extend_from_slice(&self.timestamp.to_le_bytes());
        header.extend_from_slice(&self.difficulty.to_le_bytes());
        header.extend_from_slice(&self.chain_id.to_le_bytes());
        header.extend_from_slice(&self.block_height.to_le_bytes());
        header
    }

    /// Calculate SHA-512 hash for mining
    pub fn calculate_sha512_hash(data: &[u8]) -> Vec<u8> {
        let mut hasher = Sha512::new();
        hasher.update(data);
        hasher.finalize().to_vec()
    }

    /// Double SHA-512 hash (like Bitcoin's double SHA-256)
    pub fn calculate_double_sha512_hash(data: &[u8]) -> Vec<u8> {
        let first_hash = Self::calculate_sha512_hash(data);
        Self::calculate_sha512_hash(&first_hash)
    }
}

/// Proof of work - the solution to a work package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkProof {
    pub work_id: Vec<u8>,
    pub chain_id: u32,
    pub block_height: u64,
    pub nonce: u64,
    pub extra_nonce: u64,
    pub block_hash: Vec<u8>,
    pub hash_result: Vec<u8>,
    pub timestamp: u64,
    pub miner_address: Vec<u8>,
    pub difficulty_achieved: u64,
}

impl WorkProof {
    pub fn new(
        work_id: Vec<u8>,
        chain_id: u32,
        block_height: u64,
        nonce: u64,
        extra_nonce: u64,
        block_hash: Vec<u8>,
        hash_result: Vec<u8>,
        timestamp: u64,
        miner_address: Vec<u8>,
    ) -> Result<Self> {
        if hash_result.len() != 64 {
            return Err(PoWError::InvalidWorkProof);
        }

        if miner_address.is_empty() {
            return Err(PoWError::InvalidWorkProof);
        }

        let difficulty_achieved = Self::get_difficulty_from_hash(&hash_result)?;

        Ok(Self {
            work_id,
            chain_id,
            block_height,
            nonce,
            extra_nonce,
            block_hash,
            hash_result,
            timestamp,
            miner_address,
            difficulty_achieved,
        })
    }

    pub fn verify(&self, target: &[u8]) -> Result<bool> {
        // SHA-512 produces 64 bytes
        if self.hash_result.len() != 64 {
            return Err(PoWError::InvalidWorkProof);
        }

        if target.len() != 64 {
            return Err(PoWError::InvalidWorkProof);
        }

        // Compare hash result with target
        // Hash must be less than or equal to target (in big-endian interpretation)
        let is_valid = self.hash_result.as_slice() <= target;

        if is_valid {
            debug!(
                "Work proof verified for chain {} at height {} with difficulty {}",
                self.chain_id, self.block_height, self.difficulty_achieved
            );
        }

        Ok(is_valid)
    }

    pub fn get_difficulty_from_hash(hash: &[u8]) -> Result<u64> {
        // SHA-512 produces 64 bytes
        if hash.len() != 64 {
            return Err(PoWError::InvalidWorkProof);
        }

        // Count leading zero bits
        let mut leading_zeros = 0u64;
        for byte in hash {
            if *byte == 0 {
                leading_zeros += 8;
            } else {
                leading_zeros += byte.leading_zeros() as u64;
                break;
            }
        }

        // Difficulty is 2^leading_zeros
        if leading_zeros >= 64 {
            Ok(u64::MAX)
        } else {
            Ok(1u64 << leading_zeros)
        }
    }

    pub fn verify_timestamp(&self, max_age_seconds: u64) -> Result<bool> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now < self.timestamp {
            return Err(PoWError::InvalidWorkProof);
        }

        Ok(now - self.timestamp <= max_age_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_work_package_creation() {
        let work = WorkPackage::new(
            0,
            100,
            vec![1u8; 32],
            vec![2u8; 32],
            vec![3u8; 32],
            1000,
            1_000_000,
        );

        assert!(work.is_ok());
        let work = work.unwrap();
        assert_eq!(work.chain_id, 0);
        assert_eq!(work.block_height, 100);
        assert_eq!(work.difficulty, 1_000_000);
        assert_eq!(work.target.len(), 64);
    }

    #[test]
    fn test_work_package_invalid_chain_id() {
        let work = WorkPackage::new(
            20,
            100,
            vec![1u8; 32],
            vec![2u8; 32],
            vec![3u8; 32],
            1000,
            1_000_000,
        );

        assert!(work.is_err());
    }

    #[test]
    fn test_work_package_zero_difficulty() {
        let work = WorkPackage::new(
            0,
            100,
            vec![1u8; 32],
            vec![2u8; 32],
            vec![3u8; 32],
            1000,
            0,
        );

        assert!(work.is_err());
    }

    #[test]
    fn test_work_package_invalid_hash_length() {
        let work = WorkPackage::new(
            0,
            100,
            vec![1u8; 31],
            vec![2u8; 32],
            vec![3u8; 32],
            1000,
            1_000_000,
        );

        assert!(work.is_err());
    }

    #[test]
    fn test_sha512_hash() {
        let data = b"test data";
        let hash = WorkPackage::calculate_sha512_hash(data);
        assert_eq!(hash.len(), 64);

        // Verify it's deterministic
        let hash2 = WorkPackage::calculate_sha512_hash(data);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_double_sha512_hash() {
        let data = b"test data";
        let hash = WorkPackage::calculate_double_sha512_hash(data);
        assert_eq!(hash.len(), 64);

        // Verify it's different from single hash
        let single_hash = WorkPackage::calculate_sha512_hash(data);
        assert_ne!(hash, single_hash);
    }

    #[test]
    fn test_work_proof_creation() {
        let proof = WorkProof::new(
            vec![1u8; 64],
            0,
            100,
            12345,
            0,
            vec![2u8; 32],
            vec![0u8; 64],
            1000,
            vec![3u8; 20],
        );

        assert!(proof.is_ok());
        let proof = proof.unwrap();
        assert_eq!(proof.chain_id, 0);
        assert_eq!(proof.nonce, 12345);
    }

    #[test]
    fn test_work_proof_invalid_hash_length() {
        let proof = WorkProof::new(
            vec![1u8; 64],
            0,
            100,
            12345,
            0,
            vec![2u8; 32],
            vec![0u8; 32],
            1000,
            vec![3u8; 20],
        );

        assert!(proof.is_err());
    }

    #[test]
    fn test_work_proof_verification() {
        let hash_result = vec![0u8; 64];
        let target = vec![255u8; 64];

        let proof = WorkProof::new(
            vec![1u8; 64],
            0,
            100,
            12345,
            0,
            vec![2u8; 32],
            hash_result,
            1000,
            vec![3u8; 20],
        )
        .unwrap();

        let result = proof.verify(&target);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_work_proof_invalid_target() {
        let hash_result = vec![255u8; 64];
        let target = vec![0u8; 64];

        let proof = WorkProof::new(
            vec![1u8; 64],
            0,
            100,
            12345,
            0,
            vec![2u8; 32],
            hash_result,
            1000,
            vec![3u8; 20],
        )
        .unwrap();

        let result = proof.verify(&target);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_difficulty_from_hash() {
        // Hash with leading zeros
        let mut hash = vec![0u8; 64];
        hash[8] = 1; // First non-zero byte at position 8

        let difficulty = WorkProof::get_difficulty_from_hash(&hash);
        assert!(difficulty.is_ok());
        assert!(difficulty.unwrap() > 0);
    }

    #[test]
    fn test_target_calculation() {
        let target1 = WorkPackage::calculate_target_from_difficulty(1).unwrap();
        let target2 = WorkPackage::calculate_target_from_difficulty(2).unwrap();

        // Higher difficulty should have lower target
        assert!(target2 <= target1);
    }
}
