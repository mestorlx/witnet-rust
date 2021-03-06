//! # BlocksManager actor
//!
//! This module contains the BlocksManager actor which is in charge
//! of managing the blocks of the Witnet blockchain received through
//! the protocol. Among its responsabilities are the following:
//!
//! * Initializing the chain info upon running the node for the first time and persisting it into storage [StorageManager](actors::storage_manager::StorageManager)
//! * Recovering the chain info from storage and keeping it in its state.
//! * Validating block candidates as they come from a session.
//! * Consolidating multiple block candidates for the same checkpoint into a single valid block.
//! * Putting valid blocks into storage by sending them to the storage manager actor.
//! * Having a method for letting other components get blocks by *hash* or *checkpoint*.
//! * Having a method for letting other components get the epoch of the current tip of the
//! blockchain (e.g. the last epoch field required for the handshake in the Witnet network
//! protocol).
use actix::{
    ActorFuture, Context, ContextFutureSpawner, Supervised, System, SystemService, WrapFuture,
};

use witnet_data_structures::chain::ChainInfo;

use crate::actors::{
    blocks_manager::messages::InvVectorsResult,
    storage_keys::CHAIN_KEY,
    storage_manager::{messages::Put, StorageManager},
};

use log::{debug, error, info};
use std::collections::HashMap;
use std::collections::HashSet;
use witnet_data_structures::chain::{Block, Epoch, Hash, InvVector};

use witnet_storage::{error::StorageError, storage::Storable};

use witnet_crypto::hash::calculate_sha256;
use witnet_util::error::WitnetError;

mod actor;
mod handlers;

/// Messages for BlocksManager
pub mod messages;

/// Possible errors when interacting with BlocksManager
#[derive(Debug)]
pub enum BlocksManagerError {
    /// A block being processed was already known to this node
    BlockAlreadyExists,
    /// A block does not exist
    BlockDoesNotExist,
    /// StorageError
    StorageError(WitnetError<StorageError>),
}

impl From<WitnetError<StorageError>> for BlocksManagerError {
    fn from(x: WitnetError<StorageError>) -> Self {
        BlocksManagerError::StorageError(x)
    }
}

////////////////////////////////////////////////////////////////////////////////////////
// ACTOR BASIC STRUCTURE
////////////////////////////////////////////////////////////////////////////////////////
/// BlocksManager actor
#[derive(Default)]
pub struct BlocksManager {
    /// Blockchain information data structure
    chain_info: Option<ChainInfo>,
    /// Map that relates an epoch with the hashes of the blocks for that epoch
    // One epoch can have more than one block
    epoch_to_block_hash: HashMap<Epoch, HashSet<Hash>>,
    /// Map that stores blocks by their hash
    blocks: HashMap<Hash, Block>,
}

/// Required trait for being able to retrieve BlocksManager address from registry
impl Supervised for BlocksManager {}

/// Required trait for being able to retrieve BlocksManager address from registry
impl SystemService for BlocksManager {}

/// Auxiliary methods for BlocksManager actor
impl BlocksManager {
    /// Method to persist chain_info into storage
    fn persist_chain_info(&self, ctx: &mut Context<Self>) {
        // Get StorageManager address
        let storage_manager_addr = System::current().registry().get::<StorageManager>();

        let chain_info = match self.chain_info.as_ref() {
            Some(x) => x,
            None => {
                error!("Trying to persist a None value");
                return;
            }
        };

        // Persist chain_info into storage. `AsyncContext::wait` registers
        // future within context, but context waits until this future resolves
        // before processing any other events.
        let msg = Put::from_value(CHAIN_KEY, chain_info).unwrap();
        storage_manager_addr
            .send(msg)
            .into_actor(self)
            .then(|res, _act, _ctx| {
                match res {
                    Ok(Ok(_)) => {
                        info!("BlocksManager successfully persisted chain_info into storage")
                    }
                    _ => {
                        error!("BlocksManager failed to persist chain_info into storage");
                        // FIXME(#72): handle errors
                    }
                }
                actix::fut::ok(())
            })
            .wait(ctx);
    }

    fn process_new_block(&mut self, block: Block) -> Result<Hash, BlocksManagerError> {
        // Calculate the hash of the block
        let hash = calculate_sha256(&block.to_bytes()?);

        // Check if we already have a block with that hash
        if let Some(_block) = self.blocks.get(&hash) {
            Err(BlocksManagerError::BlockAlreadyExists)
        } else {
            // This is a new block, insert it into the internal maps
            {
                // Insert the new block into the map that relates epochs to block hashes
                let beacon = &block.header.block_header.beacon;
                let hash_set = &mut self
                    .epoch_to_block_hash
                    .entry(beacon.checkpoint)
                    .or_insert_with(HashSet::new);
                hash_set.insert(hash);

                debug!(
                    "Checkpoint {} has {} blocks",
                    beacon.checkpoint,
                    hash_set.len()
                );
            }

            // Insert the new block into the map of known blocks
            self.blocks.insert(hash, block);

            Ok(hash)
        }
    }

    fn try_to_get_block(&mut self, hash: Hash) -> Result<Block, BlocksManagerError> {
        // Check if we have a block with that hash
        self.blocks.get(&hash).map_or_else(
            || Err(BlocksManagerError::BlockDoesNotExist),
            |block| Ok(block.clone()),
        )
    }

    fn discard_existing_inv_vectors(&mut self, inv_vectors: Vec<InvVector>) -> InvVectorsResult {
        // Missing inventory vectors
        let missing_inv_vectors = inv_vectors
            .into_iter()
            .filter(|inv_vector| {
                // Get hash from inventory vector
                let hash = match inv_vector {
                    InvVector::Error(hash)
                    | InvVector::Block(hash)
                    | InvVector::Tx(hash)
                    | InvVector::DataRequest(hash)
                    | InvVector::DataResult(hash) => hash,
                };

                // Add the inventory vector to the missing vectors if it is not found
                self.blocks.get(&hash).is_none()
            })
            .collect();

        Ok(missing_inv_vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_block() {
        let mut bm = BlocksManager::default();

        // Build hardcoded block
        let checkpoint = 2;
        let block_a = build_hardcoded_block(checkpoint, 99999);

        // Add block to BlocksManager
        let hash_a = bm.process_new_block(block_a.clone()).unwrap();

        // Check the block is added into the blocks map
        assert_eq!(bm.blocks.len(), 1);
        assert_eq!(bm.blocks.get(&hash_a).unwrap(), &block_a);

        // Check the block is added into the epoch-to-hash map
        assert_eq!(bm.epoch_to_block_hash.get(&checkpoint).unwrap().len(), 1);
        assert_eq!(
            bm.epoch_to_block_hash
                .get(&checkpoint)
                .unwrap()
                .iter()
                .next()
                .unwrap(),
            &hash_a
        );
    }

    #[test]
    fn add_same_block_twice() {
        let mut bm = BlocksManager::default();

        // Build hardcoded block
        let block = build_hardcoded_block(2, 99999);

        // Only the first block will be inserted
        assert!(bm.process_new_block(block.clone()).is_ok());
        assert!(bm.process_new_block(block).is_err());
        assert_eq!(bm.blocks.len(), 1);
    }

    #[test]
    fn add_blocks_same_epoch() {
        let mut bm = BlocksManager::default();

        // Build hardcoded blocks
        let checkpoint = 2;
        let block_a = build_hardcoded_block(checkpoint, 99999);
        let block_b = build_hardcoded_block(checkpoint, 12345);

        // Add blocks to the BlocksManager
        let hash_a = bm.process_new_block(block_a).unwrap();
        let hash_b = bm.process_new_block(block_b).unwrap();

        // Check that both blocks are stored in the same epoch
        assert_eq!(bm.epoch_to_block_hash.get(&checkpoint).unwrap().len(), 2);
        assert!(bm
            .epoch_to_block_hash
            .get(&checkpoint)
            .unwrap()
            .contains(&hash_a));
        assert!(bm
            .epoch_to_block_hash
            .get(&checkpoint)
            .unwrap()
            .contains(&hash_b));
    }

    #[test]
    fn get_existing_block() {
        // Create empty BlocksManager
        let mut bm = BlocksManager::default();

        // Create a hardcoded block
        let block_a = build_hardcoded_block(2, 99999);

        // Add the block to the BlocksManager
        let hash_a = bm.process_new_block(block_a.clone()).unwrap();

        // Try to get the block from the BlocksManager
        let stored_block = bm.try_to_get_block(hash_a).unwrap();

        assert_eq!(stored_block, block_a);
    }

    #[test]
    fn get_non_existent_block() {
        // Create empty BlocksManager
        let mut bm = BlocksManager::default();

        // Try to get a block with an invented hash
        let result = bm.try_to_get_block(Hash::SHA256([1; 32]));

        // Check that an error was obtained
        assert!(result.is_err());
    }

    #[test]
    fn discard_all() {
        // Create empty BlocksManager
        let mut bm = BlocksManager::default();

        // Build blocks
        let block_a = build_hardcoded_block(2, 99999);
        let block_b = build_hardcoded_block(1, 10000);
        let block_c = build_hardcoded_block(3, 72138);

        // Add blocks to the BlocksManager
        let hash_a = bm.process_new_block(block_a.clone()).unwrap();
        let hash_b = bm.process_new_block(block_b.clone()).unwrap();
        let hash_c = bm.process_new_block(block_c.clone()).unwrap();

        // Build vector of inventory vectors from hashes
        let mut inv_vectors = Vec::new();
        inv_vectors.push(InvVector::Block(hash_a));
        inv_vectors.push(InvVector::Block(hash_b));
        inv_vectors.push(InvVector::Block(hash_c));

        // Filter inventory vectors
        let missing_inv_vectors = bm.discard_existing_inv_vectors(inv_vectors).unwrap();

        // Check there is no missing inventory vector
        assert!(missing_inv_vectors.is_empty());
    }

    #[test]
    fn discard_some() {
        // Create empty BlocksManager
        let mut bm = BlocksManager::default();

        // Build blocks
        let block_a = build_hardcoded_block(2, 99999);
        let block_b = build_hardcoded_block(1, 10000);
        let block_c = build_hardcoded_block(3, 72138);

        // Add blocks to the BlocksManager
        let hash_a = bm.process_new_block(block_a.clone()).unwrap();
        let hash_b = bm.process_new_block(block_b.clone()).unwrap();
        let hash_c = bm.process_new_block(block_c.clone()).unwrap();

        // Missing inventory vector
        let missing_inv_vector = InvVector::Block(Hash::SHA256([1; 32]));

        // Build vector of inventory vectors from hashes
        let mut inv_vectors = Vec::new();
        inv_vectors.push(InvVector::Block(hash_a));
        inv_vectors.push(InvVector::Block(hash_b));
        inv_vectors.push(InvVector::Block(hash_c));
        inv_vectors.push(missing_inv_vector.clone());

        // Filter inventory vectors
        let missing_inv_vectors = bm.discard_existing_inv_vectors(inv_vectors).unwrap();

        // Check the expected missing inventory vectors
        assert_eq!(missing_inv_vectors, vec![missing_inv_vector]);
    }

    #[test]
    fn discard_none() {
        // Create empty BlocksManager
        let mut bm = BlocksManager::default();

        // Build blocks
        let block_a = build_hardcoded_block(2, 99999);
        let block_b = build_hardcoded_block(1, 10000);
        let block_c = build_hardcoded_block(3, 72138);

        // Add blocks to the BlocksManager
        bm.process_new_block(block_a.clone()).unwrap();
        bm.process_new_block(block_b.clone()).unwrap();
        bm.process_new_block(block_c.clone()).unwrap();

        // Missing inventory vector
        let missing_inv_vector_1 = InvVector::Block(Hash::SHA256([1; 32]));
        let missing_inv_vector_2 = InvVector::Block(Hash::SHA256([2; 32]));
        let missing_inv_vector_3 = InvVector::Block(Hash::SHA256([3; 32]));

        // Build vector of missing inventory vectors from hashes
        let mut inv_vectors = Vec::new();
        inv_vectors.push(missing_inv_vector_1);
        inv_vectors.push(missing_inv_vector_2);
        inv_vectors.push(missing_inv_vector_3);

        // Filter inventory vectors
        let missing_inv_vectors = bm
            .discard_existing_inv_vectors(inv_vectors.clone())
            .unwrap();

        // Check there is no missing inventory vector
        assert_eq!(missing_inv_vectors, inv_vectors);
    }

    #[cfg(test)]
    fn build_hardcoded_block(checkpoint: u32, influence: u64) -> Block {
        use witnet_data_structures::chain::*;
        Block {
            header: BlockHeaderWithProof {
                block_header: BlockHeader {
                    version: 1,
                    beacon: CheckpointBeacon {
                        checkpoint,
                        hash_prev_block: Hash::SHA256([4; 32]),
                    },
                    hash_merkle_root: Hash::SHA256([3; 32]),
                },
                proof: LeadershipProof {
                    block_sig: None,
                    influence,
                },
            },
            txn_count: 1,
            txns: vec![Transaction],
        }
    }
}
