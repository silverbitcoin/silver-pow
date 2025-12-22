//! Production-grade block validation for PoW consensus

use crate::{PoWConfig, PoWError, Result, WorkProof};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::time::SystemTime;
use tracing::{debug, info};

/// Block header for validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    pub version: u32,
    pub parent_hash: Vec<u8>,
    pub merkle_root: Vec<u8>,
    pub timestamp: u64,
    pub difficulty: u64,
    pub chain_id: u32,
    pub block_height: u64,
    pub nonce: u64,
    pub extra_nonce: u64,
}

impl BlockHeader {
    pub fn builder(
        version: u32,
        parent_hash: Vec<u8>,
        merkle_root: Vec<u8>,
        timestamp: u64,
    ) -> BlockHeaderBuilderValidator {
        BlockHeaderBuilderValidator {
            version,
            parent_hash,
            merkle_root,
            timestamp,
            difficulty: 1,
            chain_id: 0,
            block_height: 0,
            nonce: 0,
            extra_nonce: 0,
        }
    }



    pub fn hash(&self) -> Vec<u8> {
        let mut hasher = Sha512::new();
        hasher.update(self.version.to_le_bytes());
        hasher.update(&self.parent_hash);
        hasher.update(&self.merkle_root);
        hasher.update(self.timestamp.to_le_bytes());
        hasher.update(self.difficulty.to_le_bytes());
        hasher.update(self.chain_id.to_le_bytes());
        hasher.update(self.block_height.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.extra_nonce.to_le_bytes());
        hasher.finalize().to_vec()
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
}

/// Block validator
pub struct BlockValidator {
    config: PoWConfig,
}

impl BlockValidator {
    pub fn new(config: PoWConfig) -> Self {
        Self { config }
    }

    /// Validate block header
    pub fn validate_header(&self, header: &BlockHeader) -> Result<()> {
        // Validate difficulty
        if header.difficulty < self.config.min_difficulty {
            return Err(PoWError::InvalidBlock(
                format!("Difficulty {} below minimum", header.difficulty),
            ));
        }

        if header.difficulty > self.config.max_difficulty {
            return Err(PoWError::InvalidBlock(
                format!("Difficulty {} exceeds maximum", header.difficulty),
            ));
        }

        // Validate timestamp (not too far in future)
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if header.timestamp > now + 7200 {
            // 2 hours in future
            return Err(PoWError::InvalidBlock("Block timestamp too far in future".to_string()));
        }

        // Validate chain ID
        if header.chain_id >= 20 {
            return Err(PoWError::InvalidBlock("Invalid chain ID".to_string()));
        }

        // Validate version
        if header.version == 0 {
            return Err(PoWError::InvalidBlock("Invalid block version".to_string()));
        }

        debug!("Block header validation passed for chain {} height {}", header.chain_id, header.block_height);
        Ok(())
    }

    /// Validate proof of work
    pub fn validate_proof_of_work(
        &self,
        header: &BlockHeader,
        proof: &WorkProof,
    ) -> Result<bool> {
        // Verify nonce matches
        if proof.nonce != header.nonce || proof.extra_nonce != header.extra_nonce {
            return Err(PoWError::InvalidBlock("Nonce mismatch".to_string()));
        }

        // Verify chain ID matches
        if proof.chain_id != header.chain_id {
            return Err(PoWError::InvalidBlock("Chain ID mismatch".to_string()));
        }

        // Verify block height matches
        if proof.block_height != header.block_height {
            return Err(PoWError::InvalidBlock("Block height mismatch".to_string()));
        }

        // Calculate target from difficulty
        let target = self.calculate_target_from_difficulty(header.difficulty)?;

        // Verify hash meets target
        if proof.hash_result.as_slice() > target.as_slice() {
            return Err(PoWError::WorkProofVerificationFailed);
        }

        // Verify hash is correct
        let mut hasher = Sha512::new();
        let header_data = header.get_header_for_hashing();
        hasher.update(&header_data);
        hasher.update(header.nonce.to_le_bytes());
        hasher.update(header.extra_nonce.to_le_bytes());
        let calculated_hash = hasher.finalize().to_vec();

        if calculated_hash != proof.hash_result {
            return Err(PoWError::WorkProofVerificationFailed);
        }

        info!(
            "PoW validation passed for chain {} height {} with difficulty {}",
            header.chain_id, header.block_height, header.difficulty
        );
        Ok(true)
    }

    /// Calculate target from difficulty
    fn calculate_target_from_difficulty(&self, difficulty: u64) -> Result<Vec<u8>> {
        if difficulty == 0 {
            return Err(PoWError::InvalidDifficulty("Difficulty cannot be zero".to_string()));
        }

        let mut target = vec![0xFFu8; 64];

        if difficulty > 1 {
            let bits_to_shift = 64u32.saturating_sub(difficulty.ilog2());
            let bytes_to_zero = (bits_to_shift / 8) as usize;

            if bytes_to_zero < 64 {
                for byte in target.iter_mut().skip(64 - bytes_to_zero).take(bytes_to_zero) {
                    *byte = 0;
                }

                if bytes_to_zero > 0 && bytes_to_zero < 64 {
                    let bit_shift = bits_to_shift % 8;
                    if bit_shift > 0 {
                        target[64 - bytes_to_zero - 1] >>= bit_shift;
                    }
                }
            } else {
                target = vec![0u8; 64];
                target[0] = 1;
            }
        }

        Ok(target)
    }

    /// Validate block height sequence
    pub fn validate_block_height_sequence(
        &self,
        previous_height: u64,
        current_height: u64,
    ) -> Result<()> {
        if current_height != previous_height + 1 {
            return Err(PoWError::InvalidBlock(
                format!("Invalid block height sequence: {} -> {}", previous_height, current_height),
            ));
        }

        Ok(())
    }

    /// Validate block timestamp sequence
    pub fn validate_timestamp_sequence(
        &self,
        previous_timestamp: u64,
        current_timestamp: u64,
    ) -> Result<()> {
        if current_timestamp <= previous_timestamp {
            return Err(PoWError::InvalidBlock(
                "Block timestamp must be greater than previous block".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate difficulty adjustment
    pub fn validate_difficulty_adjustment(
        &self,
        previous_difficulty: u64,
        current_difficulty: u64,
    ) -> Result<()> {
        // Difficulty can change by at most 4x per adjustment
        let max_increase = previous_difficulty.saturating_mul(4);
        let min_decrease = previous_difficulty / 4;

        if current_difficulty > max_increase {
            return Err(PoWError::InvalidBlock(
                format!("Difficulty increase too large: {} -> {}", previous_difficulty, current_difficulty),
            ));
        }

        if current_difficulty < min_decrease {
            return Err(PoWError::InvalidBlock(
                format!("Difficulty decrease too large: {} -> {}", previous_difficulty, current_difficulty),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_header_creation() {
        let header = BlockHeader::builder(
            1,
            vec![1u8; 32],
            vec![2u8; 32],
            1000,
        )
        .with_difficulty(1_000_000)
        .with_chain_id(0)
        .with_block_height(100)
        .with_nonce(12345)
        .with_extra_nonce(0)
        .build();

        assert!(header.is_ok());
        let header = header.unwrap();
        assert_eq!(header.block_height, 100);
    }

    #[test]
    fn test_block_header_invalid_parent_hash() {
        let header = BlockHeader::builder(
            1,
            vec![1u8; 31],
            vec![2u8; 32],
            1000,
        )
        .with_difficulty(1_000_000)
        .build();

        assert!(header.is_err());
    }

    #[test]
    fn test_block_header_hash() {
        let header = BlockHeader::builder(
            1,
            vec![1u8; 32],
            vec![2u8; 32],
            1000,
        )
        .with_difficulty(1_000_000)
        .with_block_height(100)
        .with_nonce(12345)
        .build()
        .unwrap();

        let hash = header.hash();
        assert_eq!(hash.len(), 64);

        // Verify deterministic
        let hash2 = header.hash();
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_block_validator_creation() {
        let config = PoWConfig::default();
        let validator = BlockValidator::new(config);
        assert_eq!(validator.config.mining_algorithm, "SHA-512");
    }

    #[test]
    fn test_validate_header() {
        let config = PoWConfig::default();
        let validator = BlockValidator::new(config);

        let header = BlockHeader::builder(
            1,
            vec![1u8; 32],
            vec![2u8; 32],
            1000,
        )
        .with_difficulty(1_000_000)
        .with_block_height(100)
        .with_nonce(12345)
        .build()
        .unwrap();

        assert!(validator.validate_header(&header).is_ok());
    }

    #[test]
    fn test_validate_header_invalid_difficulty() {
        let header = BlockHeader::builder(
            1,
            vec![1u8; 32],
            vec![2u8; 32],
            1000,
        )
        .with_difficulty(0)
        .build();

        assert!(header.is_err());
    }

    #[test]
    fn test_validate_block_height_sequence() {
        let config = PoWConfig::default();
        let validator = BlockValidator::new(config);

        assert!(validator.validate_block_height_sequence(99, 100).is_ok());
        assert!(validator.validate_block_height_sequence(99, 101).is_err());
    }

    #[test]
    fn test_validate_timestamp_sequence() {
        let config = PoWConfig::default();
        let validator = BlockValidator::new(config);

        assert!(validator.validate_timestamp_sequence(1000, 2000).is_ok());
        assert!(validator.validate_timestamp_sequence(2000, 1000).is_err());
    }

    #[test]
    fn test_validate_difficulty_adjustment() {
        let config = PoWConfig::default();
        let validator = BlockValidator::new(config);

        let base_diff = 1_000_000u64;

        // Valid adjustments
        assert!(validator.validate_difficulty_adjustment(base_diff, base_diff * 2).is_ok());
        assert!(validator.validate_difficulty_adjustment(base_diff, base_diff / 2).is_ok());

        // Invalid adjustments
        assert!(validator.validate_difficulty_adjustment(base_diff, base_diff * 5).is_err());
        assert!(validator.validate_difficulty_adjustment(base_diff, base_diff / 5).is_err());
    }
}

/// Builder for BlockHeader - real production-grade builder pattern
pub struct BlockHeaderBuilderValidator {
    version: u32,
    parent_hash: Vec<u8>,
    merkle_root: Vec<u8>,
    timestamp: u64,
    difficulty: u64,
    chain_id: u32,
    block_height: u64,
    nonce: u64,
    extra_nonce: u64,
}

impl BlockHeaderBuilderValidator {
    pub fn with_difficulty(mut self, difficulty: u64) -> Self {
        self.difficulty = difficulty;
        self
    }

    pub fn with_chain_id(mut self, chain_id: u32) -> Self {
        self.chain_id = chain_id;
        self
    }

    pub fn with_block_height(mut self, height: u64) -> Self {
        self.block_height = height;
        self
    }

    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }

    pub fn with_extra_nonce(mut self, extra_nonce: u64) -> Self {
        self.extra_nonce = extra_nonce;
        self
    }

    pub fn build(self) -> Result<BlockHeader> {
        if self.parent_hash.len() != 32 {
            return Err(PoWError::InvalidBlock("Invalid parent hash length".to_string()));
        }

        if self.merkle_root.len() != 32 {
            return Err(PoWError::InvalidBlock("Invalid merkle root length".to_string()));
        }

        if self.difficulty == 0 {
            return Err(PoWError::InvalidBlock("Difficulty cannot be zero".to_string()));
        }

        Ok(BlockHeader {
            version: self.version,
            parent_hash: self.parent_hash,
            merkle_root: self.merkle_root,
            timestamp: self.timestamp,
            difficulty: self.difficulty,
            chain_id: self.chain_id,
            block_height: self.block_height,
            nonce: self.nonce,
            extra_nonce: self.extra_nonce,
        })
    }
}
