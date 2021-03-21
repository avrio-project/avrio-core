extern crate avrio_config;
extern crate avrio_core;
extern crate avrio_database;
use crate::genesis::{genesisBlockErrors, getGenesisBlock};
use avrio_config::config;
use avrio_core::{
    account::{getAccount, setAccount, Account},
    transaction::*,
};
use avrio_database::*;
use serde::{Deserialize, Serialize};
#[macro_use]
extern crate log;

extern crate bs58;

use ring::{
    rand as randc,
    signature::{self, KeyPair},
};
extern crate rand;

use avrio_crypto::Hashable;

use std::fs::File;
use std::io::prelude::*;

#[derive(Debug)]
pub enum blockValidationErrors {
    invalidBlockhash,
    badSignature,
    indexMissmatch,
    invalidPreviousBlockhash,
    invalidTransaction(TransactionValidationErrors),
    genesisBlockMissmatch,
    failedToGetGenesisBlock,
    blockExists,
    tooLittleSignatures,
    badNodeSignature,
    timestampInvalid,
    networkMissmatch,
    other,
}
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum BlockType {
    Send,
    Recieve,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct Header {
    pub version_major: u8,
    pub version_breaking: u8,
    pub version_minor: u8,
    pub chain_key: String,
    pub prev_hash: String,
    pub height: u64,
    pub timestamp: u64,
    pub network: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct Block {
    pub header: Header,
    pub block_type: BlockType,
    pub send_block: Option<String>, // the send block this recieve block is in refrence to
    pub txns: Vec<Transaction>,
    pub hash: String,
    pub signature: String,
    pub confimed: bool,
    pub node_signatures: Vec<BlockSignature>, // a block must be signed by at least 2/3 of the commitee's verifyer nodes to be valid (ensures at least one honest node has signed it)
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct BlockSignature {
    /// The signature of the vote
    pub hash: String,
    /// The timestamp at which the signature was created
    pub timestamp: u64,
    /// The hash of the block this signature is about        
    pub block_hash: String,
    /// The public key of the node which created this vote
    pub signer_public_key: String,
    /// The hash of the sig signed by the voter        
    pub signature: String,
    /// A nonce to prevent sig replay attacks
    pub nonce: u64,
}

impl Hashable for BlockSignature {
    fn bytes(&self) -> Vec<u8> {
        let mut bytes: Vec<u8> = vec![];
        write!(bytes, "{}", self.timestamp).unwrap();
        bytes.extend(self.block_hash.as_bytes());
        bytes.extend(self.signer_public_key.as_bytes());
        write!(bytes, "{}", self.nonce).unwrap();
        bytes
    }
}

impl Default for BlockType {
    fn default() -> Self {
        BlockType::Send
    }
}

impl BlockSignature {
    pub fn enact(&self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        // we are presuming the vote is valid - if it is not this is going to mess stuff up!
        if save_data(
            self.nonce.to_string(),
            config().db_path + "/fn-certificates",
            self.signer_public_key.clone(),
        ) != 1
        {
            return Err("failed to update nonce".into());
        } else {
            return Ok(());
        }
    }
    pub fn valid(&self) -> bool {
        if &get_data(
            config().db_path + "/fn-certificates",
            &self.signer_public_key,
        ) == "-1"
        // check the fullnode who signed this block is registered. TODO (for sharding v1): move to using a vector of tuples(publickey, signature) and check each fullnode fully (was part of that epoch, was a validator node for the commitee handling the shard, etc)
        {
            return false;
        } else if self.hash != self.hash_return() {
            return false;
        } else if get_data(
            config().db_path + "/chains/" + &self.signer_public_key + "-chainindex",
            "sigcount",
        ) != self.nonce.to_string()
        {
            return false;
        } else if self.timestamp - (config().transaction_timestamp_max_offset as u64)
            < (SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis() as u64)
            || self.timestamp + (config().transaction_timestamp_max_offset as u64)
                < (SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_millis() as u64)
        {
            return false;
        } else if !self.signature_valid() {
            return false;
        } else {
            return true;
        }
    }
    pub fn hash(&mut self) {
        self.hash = self.hash_item();
    }
    pub fn hash_return(&self) -> String {
        return self.hash_item();
    }
    pub fn sign(
        &mut self,
        private_key: String,
    ) -> std::result::Result<(), ring::error::KeyRejected> {
        let key_pair = signature::Ed25519KeyPair::from_pkcs8(
            bs58::decode(private_key)
                .into_vec()
                .unwrap_or_default()
                .as_ref(),
        )?;
        let msg: &[u8] = self.hash.as_bytes();
        self.signature = bs58::encode(key_pair.sign(msg)).into_string();
        return Ok(());
    }
    pub fn bytes_all(&self) -> Vec<u8> {
        let mut bytes: Vec<u8> = vec![];
        bytes.extend(self.hash.as_bytes());
        bytes.extend(self.timestamp.to_string().as_bytes());
        bytes.extend(self.block_hash.as_bytes());
        bytes.extend(self.signer_public_key.as_bytes());
        bytes.extend(self.nonce.to_string().as_bytes());
        bytes.extend(self.signature.as_bytes());
        bytes
    }
    pub fn signature_valid(&self) -> bool {
        let msg: &[u8] = self.hash.as_bytes();
        let peer_public_key = signature::UnparsedPublicKey::new(
            &signature::ED25519,
            bs58::decode(&self.signer_public_key)
                .into_vec()
                .unwrap_or_else(|e| {
                    error!(
                        "Failed to decode public key from bs58 {}, gave error {}",
                        self.signer_public_key, e
                    );
                    return vec![0, 1, 0];
                }),
        );
        let mut res: bool = true;
        peer_public_key
            .verify(
                msg,
                bs58::decode(&self.signature)
                    .into_vec()
                    .unwrap_or_else(|e| {
                        error!(
                            "failed to decode signature from bs58 {}, gave error {}",
                            self.signature, e
                        );
                        return vec![0, 1, 0];
                    })
                    .as_ref(),
            )
            .unwrap_or_else(|_e| {
                res = false;
            });
        return res;
    }
}
/*
pub fn generate_merkle_root_all() -> std::result::Result<String, Box<dyn std::error::Error>> {
    trace!(target: "blockchain::chain_digest","Generating state digest from scratch");
    let _roots: Vec<String> = vec![];
    if let Ok(db) = open_database(config().db_path + "/chainlist") {
        let mut iter = db.raw_iterator();
        iter.seek_to_first();
        let cd_db = open_database(config().db_path + &"/chaindigest".to_owned()).unwrap();
        let mut chains_list: Vec<String> = Vec::new();
        while iter.valid() {
            if let Some(chain) = iter.key() {
                if let Ok(chain_string) = String::from_utf8(chain.to_vec()) {
                    chains_list.push(chain_string);
                }
            }
            iter.next();
        }
        chains_list.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        for chain_string in chains_list {
            if let Ok(blkdb) =
                open_database(config().db_path + "/chains/" + &chain_string + "-chainindex")
            {
                let mut blkiter = blkdb.raw_iterator();
                blkiter.seek_to_first();
                while blkiter.valid() {
                    if let Some(blk) = iter.value() {
                        let s: String = String::from_utf8(blk.to_vec())?;
                        if let Ok(_) = String::from_utf8(iter.key().unwrap_or_default().to_vec())?
                            .parse::<u64>()
                        {
                            trace!(target: "blockchain::chain_digest","Chain digest: {}", update_chain_digest(&s, &cd_db));
                        }
                    }
                    blkiter.next();
                }
            }
        }
    }
    return Ok(get_data(config().db_path + &"/chainsdigest", "master"));
}
*/

pub fn update_chain_digest(new_blk_hash: &String, cd_db: &rocksdb::DB, chain: &String) -> String {
    trace!(target: "blockchain::chain_digest","Updating chain digest for chain={}, hash={}", chain, new_blk_hash);
    let curr = get_data_from_database(cd_db, chain);
    let root: String;
    if &curr == "-1" {
        trace!(target: "blockchain::chain_digest","chain digest not set");
        root = avrio_crypto::raw_lyra(new_blk_hash);
    } else {
        trace!(target: "blockchain::chain_digest","Updating set chain digest. Curr: {}", curr);
        root = avrio_crypto::raw_lyra(&(curr + new_blk_hash));
    }
    let _ = set_data_in_database(&root, &cd_db, chain);
    trace!(target: "blockchain::chain_digest","Chain digest for chain={} updated to {}", chain, root);
    return root;
}

/// takes a DB object of the chains digest (chaindigest) db and a vector of chain_keys (as strings) and calculates the chain digest for each chain.
/// It then sets the value of chain digest (for each chain) in the db, and returns it in the vector of strings
pub fn form_chain_digest(
    cd_db: &rocksdb::DB,
    chains: Vec<String>,
) -> std::result::Result<Vec<String>, Box<dyn std::error::Error>> {
    // TODO: do we need to return a Result<vec, err>? Cant we just return vec as there is no unwrapping?
    let mut output: Vec<String> = vec![];
    for chain in chains {
        trace!("Chain digest: starting chain={}", chain);
        // get the genesis block
        let genesis = getBlock(&chain, 0);
        // get the first non genesis block (height=1, will be the recieve block for the genesis block)s
        let block_one = getBlock(&chain, 1);
        let mut curr_height: u64 = 2;
        // hash them together to get the first temp_leaf node
        let mut temp_leaf =
            avrio_crypto::raw_lyra(&(avrio_crypto::raw_lyra(&genesis.hash) + &block_one.hash));
        loop {
            // loop through, increasing curr_height by one each time. Get block with height curr_height and hash its hash with the previous temp_leaf node. Once the block we read at curr_height
            // is Default (eg there is no block at that height), break from the loop
            let temp_block = getBlock(&chain, curr_height);
            if temp_block.is_default() {
                break; // we have exceeded the last block, break/return from loop
            } else {
                temp_leaf = avrio_crypto::raw_lyra(&(temp_leaf + &temp_block.hash));
                trace!(
                    "Chain digest: chain={}, block={}, height={}, new temp_leaf={}",
                    chain,
                    temp_block.hash,
                    curr_height,
                    temp_leaf
                );
                curr_height += 1;
            }
        }
        // we are finished, update the chain_digest on disk and add it to the output vector
        avrio_database::set_data_in_database(&temp_leaf, cd_db, &chain);
        output.push(temp_leaf);
        trace!(
            "Chain digest: Finished chain={}, new output={:?}",
            chain,
            output
        );
    }
    // return the output vector
    return Ok(output);
}

/// Calculates the 'overall' digest of the DAG.
/// Pass it a database object of the chaindigest database. This database should contain all the chains chain digests (with the key being the publickey)
/// as well as 'master' (as a key) being the state digest.
/// Run form_chain_digest(chain) (with chain being the publickey of the chain you want, or * for every chain) first which will form a chain digest
/// from scratch (or update_chain_digest(chain, new_block_hash, cd_db)). This function will return the new state digest as a string as well as update it in the database
///
pub fn form_state_digest(
    cd_db: &rocksdb::DB,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    debug!("Updating state digest");
    let start = std::time::Instant::now();
    let current_state_digest = get_data_from_database(cd_db, "master"); // get the current state digest, for refrence
    if &current_state_digest == "-1" {
        trace!("State digest not set");
    } else {
        trace!("Updating set state digest. Curr: {}", current_state_digest);
    }
    // we now recursivley loop through cd_db and add every value (other than master) to a vector
    // now we have every chain digest in a vector we sort it alphabeticly
    // now the vector of chain digests is sorted alphabeticly we recursivley hash them
    // like so: (TODO: use a merkle tree not a recursive hash chain)
    // leaf_one = hash(chain_digest_one + chain_digest_two)
    // leaf_two = hash(leaf_one + chain_digest_three)
    // leaf[n] = hash(leaf[n-1] + chain_digest[n+1])
    let mut _roots: Vec<(String, String)> = vec![]; // 0: chain_key, 1: chain_digest
    let mut iter = cd_db.raw_iterator();
    iter.seek_to_first();
    let _chains_list: Vec<String> = Vec::new();
    while iter.valid() {
        if let Some(chain_digest) = iter.value() {
            if let Ok(chain_digest_string) = String::from_utf8(chain_digest.to_vec()) {
                if let Some(chain_key) = iter.key() {
                    if let Ok(chain_key_string) = String::from_utf8(chain_key.to_vec()) {
                        if chain_key_string != "master" && chain_key_string != "blockcount" {
                            _roots.push((chain_key_string, chain_digest_string));
                        } else {
                            log::trace!(
                                "found {}:{} (key, value) in chaindigest database, ignoring",
                                chain_key_string,
                                chain_digest_string
                            );
                        }
                    }
                }
                //chains_list.push(chain_string);
            }
        }
        iter.next();
    }
    let _rootsps = _roots.clone();
    _roots.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase())); // sort to aplabetical order (based on chain key)
    log::trace!(
        "Roots presort={:#?}, roots post sort={:#?}",
        _rootsps,
        _roots
    );
    drop(_rootsps);
    let mut temp_leaf: String;
    // create the first leaf
    if _roots.len() != 0 {
        temp_leaf = avrio_crypto::raw_lyra(&(_roots[0].1.to_owned() + &_roots[1].1)); // Hash the first two chain digests together to make the first leaf
        let cd_one = &_roots[0].1;
        let cd_two = &_roots[1].1;
        for (chain_string, digest_string) in _roots.clone() {
            // TODO: can we put _roots in a cow (std::borrow::Cow) to prevent cloning? (micro-optimisation)
            // check that digest_string is not the first two (which we already hashed)
            if &digest_string == cd_one || &digest_string == cd_two {
            } else {
                // hash digest_string with temp_leaf
                log::trace!(
                    "Chain digest: chain={}, chain_digest={}, current_tempory_leaf={}",
                    chain_string,
                    digest_string,
                    temp_leaf
                );
                temp_leaf = avrio_crypto::raw_lyra(&(digest_string + &temp_leaf));
            }
        }
        // we have gone through every digest and hashed them together, now we save to disk
    } else if _roots.len() == 1 {
        temp_leaf = avrio_crypto::raw_lyra(_roots[0].1.to_owned());
    } else {
        temp_leaf = avrio_crypto::raw_lyra(&"".to_owned());
    }
    log::debug!(
        "Finished state digest calculation, old={}, new={}, time_to_complete={}",
        current_state_digest,
        temp_leaf,
        start.elapsed().as_millis()
    );
    avrio_database::set_data_in_database(&temp_leaf, cd_db, &"master");
    return Ok(temp_leaf.into());
}

/// returns the block when you know the chain and the height
pub fn getBlock(chainkey: &String, height: u64) -> Block {
    let hash = get_data(
        config().db_path + "/chains/" + chainkey + "-chainindex",
        &height.to_string(),
    );
    if hash == "-1".to_owned() {
        return Block::default();
    } else if hash == "0".to_owned() {
        return Block::default();
    } else {
        return getBlockFromRaw(hash);
    }
}

/// returns the block when you only know the hash by opeining the raw blk-HASH.dat file (where hash == the block hash)
pub fn getBlockFromRaw(hash: String) -> Block {
    let try_open = File::open(config().db_path + &"/blocks/blk-".to_owned() + &hash + ".dat");
    if let Ok(mut file) = try_open {
        let mut contents = String::new();
        file.read_to_string(&mut contents);
        return serde_json::from_str(&contents).unwrap_or_default();
    } else {
        trace!(
            "Opening raw block file (hash={}) failed. Reason={}",
            hash,
            try_open.unwrap_err()
        );
        return Block::default();
    }
}

/// formats the block into a .dat file and saves it under block-hash.dat
pub fn saveBlock(block: Block) -> std::result::Result<(), Box<dyn std::error::Error>> {
    trace!("Saving block with hash: {}", block.hash);
    let encoded: Vec<u8> = serde_json::to_string(&block)?.as_bytes().to_vec();
    let mut file = File::create(config().db_path + "/blocks/blk-" + &block.hash + ".dat")?;
    file.write_all(&encoded)?;
    trace!("Saved Block");
    return Ok(());
}

impl Hashable for Header {
    fn bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];

        bytes.extend(self.version_major.to_string().as_bytes());
        bytes.extend(self.version_breaking.to_string().as_bytes());
        bytes.extend(self.version_minor.to_string().as_bytes());
        bytes.extend(self.chain_key.as_bytes());
        bytes.extend(self.prev_hash.as_bytes());
        bytes.extend(self.height.to_string().as_bytes());
        bytes.extend(self.timestamp.to_string().as_bytes());
        bytes
    }
}
impl Header {
    /// Returns the hash of the header bytes
    pub fn hash(&mut self) -> String {
        return self.hash_item();
    }
}

impl Hashable for Block {
    fn bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];

        bytes.extend(self.header.bytes());
        for tx in self.txns.clone() {
            bytes.extend(tx.hash.as_bytes());
        }
        bytes
    }
}
impl Block {
    pub fn is_default(&self) -> bool {
        self == &Block::default()
    }
    /// Sets the hash of a block
    pub fn hash(&mut self) {
        self.hash = self.hash_item();
    }
    /// Returns the hash of a block
    pub fn hash_return(&self) -> String {
        return self.hash_item();
    }
    /// Signs a block and sets the signature field on it.
    /// Returns a Result enum
    pub fn sign(
        &mut self,
        private_key: &String,
    ) -> std::result::Result<(), ring::error::KeyRejected> {
        let key_pair = signature::Ed25519KeyPair::from_pkcs8(
            bs58::decode(private_key).into_vec().unwrap().as_ref(),
        )?;
        let msg: &[u8] = self.hash.as_bytes();
        self.signature = bs58::encode(key_pair.sign(msg)).into_string();
        return Ok(());
    }
    /// Returns true if signature on block is valid
    pub fn validSignature(&self) -> bool {
        let msg: &[u8] = self.hash.as_bytes();
        let peer_public_key = signature::UnparsedPublicKey::new(
            &signature::ED25519,
            bs58::decode(&self.header.chain_key)
                .into_vec()
                .unwrap_or_else(|e| {
                    error!(
                        "Failed to decode public key from bs58 {}, gave error {}",
                        self.header.chain_key, e
                    );
                    return vec![0, 1, 0];
                }),
        );
        let mut res: bool = true;
        peer_public_key
            .verify(
                msg,
                bs58::decode(&self.signature)
                    .into_vec()
                    .unwrap_or_else(|e| {
                        error!(
                            "failed to decode signature from bs58 {}, gave error {}",
                            self.signature, e
                        );
                        return vec![0, 1, 0];
                    })
                    .as_ref(),
            )
            .unwrap_or_else(|_e| {
                res = false;
            });
        return res;
    }
    pub fn isOtherBlock(&self, OtherBlock: &Block) -> bool {
        self == OtherBlock
    }
    /// Takes in a send block and creates and returns a recive block
    pub fn form_receive_block(
        &self,
        chain_key: Option<String>,
    ) -> Result<Block, Box<dyn std::error::Error>> {
        if self.block_type == BlockType::Recieve {
            return Err("Block is recive block already".into());
        }
        // else we can get on with forming the rec block for this block
        let mut blk_clone = self.clone();
        blk_clone.block_type = BlockType::Recieve;
        let mut chainKey: String = config().chain_key;
        if let Some(key) = chain_key {
            chainKey = key;
        }
        let txn_iter = 0;
        for txn in blk_clone.clone().txns {
            txn_iter + 1;
            if txn.receive_key != chainKey {
                blk_clone.txns.remove(txn_iter);
            }
        }
        if chainKey == self.header.chain_key {
            blk_clone.header.height += 1;
            blk_clone.send_block = Some(self.hash.to_owned());
            blk_clone.header.prev_hash = self.hash.clone();
            blk_clone.hash();
            return Ok(blk_clone);
        } else {
            let top_block_hash = get_data(
                config().db_path + &"/chains/".to_owned() + &chainKey + &"-chainindex".to_owned(),
                "topblockhash",
            );
            let our_height: u64;
            let our_height_ = get_data(
                config().db_path + &"/chains/".to_owned() + &chainKey + &"-chainindex".to_owned(),
                &"blockcount".to_owned(),
            );
            if our_height_ == "-1" {
                our_height = 0
            } else {
                our_height = our_height_.parse()?;
            }
            blk_clone.header.chain_key = chainKey;
            blk_clone.header.height = our_height + 1;
            blk_clone.send_block = Some(self.hash.to_owned());
            blk_clone.header.prev_hash = top_block_hash;
            blk_clone.hash();
            return Ok(blk_clone);
        }
    }
}
/// enacts the relevant stuff for a send block (eg creating inv registry)
pub fn enact_send(block: Block) -> Result<(), Box<dyn std::error::Error>> {
    let chaindex_db = open_database(
        config().db_path
            + &"/chains/".to_owned()
            + &block.header.chain_key
            + &"-chainindex".to_owned(),
    )
    .unwrap();

    if get_data_from_database(&chaindex_db, &block.header.height.to_string()) == "-1" {
        debug!("block not in invs");

        let hash = block.hash.clone();

        use std::sync::Arc;

        let arc_db =
            Arc::new(open_database(config().db_path + &"/chaindigest".to_owned()).unwrap());
        let arc = arc_db.clone();

        let chain_key_copy = block.header.chain_key.to_owned();
        std::thread::spawn(move || {
            update_chain_digest(&hash, &arc, &chain_key_copy);
            form_state_digest(&arc);
        });

        set_data_in_database(&block.hash, &chaindex_db, &"topblockhash");
        set_data_in_database(
            &(block.header.height + 1).to_string(),
            &chaindex_db,
            &"blockcount",
        );

        trace!("set top block hash for sender");

        let inv_sender_res =
            set_data_in_database(&block.hash, &chaindex_db, &block.header.height.to_string());

        trace!("Saved inv for sender: {}", block.header.chain_key);

        if inv_sender_res != 1 {
            return Err("failed to save sender inv".into());
        }

        let block_count = get_data_from_database(&arc_db, &"blockcount");

        if block_count == "-1".to_owned() {
            set_data_in_database(&"1".to_owned(), &arc_db, &"blockcount");
            trace!("set block count, prev: -1 (not set), new: 1");
        } else {
            let mut bc: u64 = block_count.parse().unwrap_or_default();
            bc += 1;
            set_data_in_database(&bc.to_string(), &arc_db, &"blockcount");
            trace!("Updated non-zero block count, new count: {}", bc);
        }

        if block.header.height == 0 {
            if save_data(
                "".to_owned(),
                config().db_path + "/chainlist",
                block.header.chain_key.clone(),
            ) == 0
            {
                return Err("failed to add chain to chainslist".into());
            } else {
                let newacc = Account::new(block.header.chain_key.clone());

                if setAccount(&newacc) != 1 {
                    return Err("failed to save new account".into());
                }
            }

            if avrio_database::get_data_from_database(&chaindex_db, &"txncount") == "-1".to_owned()
            {
                avrio_database::set_data_in_database(&"0".to_string(), &chaindex_db, &"txncount");
            }
        }
    }
    return Ok(());
}
// TODO: finish enact block
/// Enacts a recieve block. Updates all relavant dbs and files
/// You should not enact a send block (this will return an error).
/// In presharding networks (eg now) use enact_send then form_receive_block and enact the outputed recieve block
/// Make sure the send block is propegated BEFORE the recieve block (to reduce processing latency)
pub fn enact_block(block: Block) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if block.block_type != BlockType::Recieve && block.header.height != 0 {
        // we only enact recive blocks, ignore send blocks
        return Err("tried to enact a send block".into());
    }

    let chaindex_db = open_database(
        config().db_path
            + &"/chains/".to_owned()
            + &block.header.chain_key
            + &"-chainindex".to_owned(),
    )
    .unwrap();
    if get_data_from_database(&chaindex_db, &block.header.height.to_string()) == "-1" {
        debug!("block not in invs");
        let hash = block.hash.clone();
        use std::sync::Arc;
        let arc_db =
            Arc::new(open_database(config().db_path + &"/chaindigest".to_owned()).unwrap());
        let arc = arc_db.clone();
        let chain_key_copy = block.header.chain_key.to_owned();
        std::thread::spawn(move || {
            update_chain_digest(&hash, &arc, &chain_key_copy);
            form_state_digest(&arc);
        });
        set_data_in_database(&block.hash, &chaindex_db, &"topblockhash");
        set_data_in_database(
            &(block.header.height + 1).to_string(),
            &chaindex_db,
            &"blockcount",
        );
        trace!("set top block hash for sender");
        let inv_sender_res =
            set_data_in_database(&block.hash, &chaindex_db, &block.header.height.to_string());
        trace!("Saved inv for sender: {}", block.header.chain_key);
        if inv_sender_res != 1 {
            return Err("failed to save sender inv".into());
        }
        let block_count = get_data_from_database(&arc_db, &"blockcount");
        if block_count == "-1".to_owned() {
            set_data_in_database(&"1".to_owned(), &arc_db, &"blockcount");
            trace!("set block count, prev: -1 (not set), new: 1");
        } else {
            let mut bc: u64 = block_count.parse().unwrap_or_default();
            bc += 1;
            set_data_in_database(&bc.to_string(), &arc_db, &"blockcount");
            trace!("Updated non-zero block count, new count: {}", bc);
        }
        if block.header.height == 0 {
            if save_data(
                "".to_owned(),
                config().db_path + "/chainlist",
                block.header.chain_key.clone(),
            ) == 0
            {
                return Err("failed to add chain to chainslist".into());
            } else {
                let newacc = Account::new(block.header.chain_key.clone());
                if setAccount(&newacc) != 1 {
                    return Err("failed to save new account".into());
                }
            }
            if avrio_database::get_data_from_database(&chaindex_db, &"txncount") == "-1".to_owned()
            {
                avrio_database::set_data_in_database(&"0".to_string(), &chaindex_db, &"txncount");
            }
        }
        let txn_db = open_database(config().db_path + &"/transactions".to_owned()).unwrap();
        for txn in block.txns {
            trace!("enacting txn with hash: {}", txn.hash);
            txn.enact(&chaindex_db)?;
            trace!("Enacted txn. Saving txn to txindex db (db_name  = transactions)");
            if set_data_in_database(&block.hash, &txn_db, &txn.hash) != 1 {
                return Err("failed to save txn in transactions db".into());
            }
            trace!("Saving invs");
            if txn.sender_key != txn.receive_key && txn.sender_key != block.header.chain_key {
                let rec_db = open_database(
                    config().db_path
                        + &"/chains/".to_owned()
                        + &txn.receive_key
                        + &"-chainindex".to_owned(),
                )
                .unwrap();
                let inv_receiver_res =
                    set_data_in_database(&block.hash, &rec_db, &block.header.height.to_string());
                if inv_receiver_res != 1 {
                    return Err("failed to save reciver inv".into());
                }
                let curr_block_count: String = get_data_from_database(&rec_db, &"blockcount");
                if curr_block_count == "-1" {
                    set_data_in_database(&"0".to_owned(), &rec_db, &"blockcount");
                } else {
                    let curr_block_count_val: u64 = curr_block_count.parse().unwrap_or_default();
                    set_data_in_database(
                        &(curr_block_count_val + 1).to_string(),
                        &rec_db,
                        &"blockcount",
                    );
                }

                set_data_in_database(&block.hash, &rec_db, &"topblockhash");
                trace!("set top block hash for reciever");
                drop(rec_db);
            }
        }
        drop(txn_db);
        drop(chaindex_db);
    } else {
        debug!("Block in invs, ignoring");
    }
    return Ok(());
}

/// Checks if a block is valid returns a blockValidationErrors
pub fn check_block(blk: Block) -> std::result::Result<(), blockValidationErrors> {
    let got_block = getBlockFromRaw(blk.hash.clone()); // try to read this block from disk, if it is saved it is assumed to already have been vaildated and hence is not revalidated
    if got_block == blk {
        // we have this block stored as a raw file (its valid)
        return Ok(());
    } else if got_block == Block::default() {
        // we dont have this block in raw block storage files; validate it
        if blk.header.network != config().network_id {
            // check this block originated from the same network as us
            return Err(blockValidationErrors::networkMissmatch);
        } else if blk.hash != blk.hash_return() {
            // hash the block and compare it to the claimed hash of the block.
            trace!(
                "Hash missmatch block: {}, computed hash: {}",
                blk.hash,
                blk.hash_return()
            );
            return Err(blockValidationErrors::invalidBlockhash);
        }
        if get_data(config().db_path + "/checkpoints", &blk.hash) != "-1".to_owned() {
            // we have this block in our checkpoints db and we know the hash is correct and therefore the block is valid
            return Ok(());
        }
        if blk.header.height == 0 {
            // This is a genesis block (the first block of a chain)
            // First we will check if there is a entry for this chain in the genesis blocks db
            let genesis: Block;
            let mut is_in_db = false;
            match getGenesisBlock(&blk.header.chain_key) {
                Ok(b) => {
                    // this is in our genesis block db and so is a swap block (pregenerated to swap coins from the old network)
                    trace!("found genesis block in db");
                    genesis = b;
                    is_in_db = true;
                }
                Err(e) => match e {
                    genesisBlockErrors::BlockNotFound => {
                        // this block is not in the genesis block db therefor this is a new chain that is not from the swap
                        genesis = Block::default();
                        is_in_db = false;
                    }
                    _ => {
                        warn!(
                            "Failed to get genesis block for chain: {}, gave error: {:?}",
                            &blk.header.chain_key, e
                        );
                        return Err(blockValidationErrors::failedToGetGenesisBlock);
                    }
                },
            }
            if blk != genesis && genesis != Block::default() {
                trace!(
                    "Genesis blocks missmatch. Ours: {:?}, propsed: {:?}",
                    genesis,
                    blk
                );
                return Err(blockValidationErrors::genesisBlockMissmatch);
            } else {
                if is_in_db == true {
                    // if it is in the genesis block db it is guarenteed to be valid (as its pregenerated), we do not need to validate the block
                    return Ok(());
                } else {
                    // if it isn't it needs to be validated like any other block
                    if &blk.header.prev_hash != "00000000000" {
                        // genesis blocks should always reference "00000000000" as a previous hash (as there is none)
                        return Err(blockValidationErrors::invalidPreviousBlockhash);
                    } else if let Ok(acc) = getAccount(&blk.header.chain_key) {
                        // this account already exists, you can't have two genesis blocks
                        trace!("Already got acccount: {:?}", acc);
                        return Err(blockValidationErrors::genesisBlockMissmatch);
                    } else if !blk.validSignature() {
                        return Err(blockValidationErrors::badSignature);
                    } else if getBlockFromRaw(blk.hash.clone()) != Block::default() {
                        // this block already exists; this will return if the block is enacted and saved before running this function on a genesis block. So dont. :)
                        return Err(blockValidationErrors::blockExists);
                    } else if blk.header.height != 0 // genesis blocks are exempt from broadcast delta limmits
                        && blk.header.timestamp - (config().transaction_timestamp_max_offset as u64)
                            > (SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .expect("Time went backwards")
                                .as_millis() as u64)
                    {
                        // this block is too far in the future
                        return Err(blockValidationErrors::timestampInvalid);
                    } else if blk.header.height != 0
                        && getBlockFromRaw(blk.header.prev_hash).header.timestamp
                            > blk.header.timestamp
                    {
                        // this block is older than its parent (prev block hash)
                        debug!("Block: {} timestamp under previous timestamp", blk.hash);
                        return Err(blockValidationErrors::timestampInvalid);
                    }
                    // if you got here the block is valid, yay!
                    return Ok(());
                }
            }
        } else {
            // not genesis block
            if blk.confimed == true
                && blk.node_signatures.len() < (2 / 3 * config().commitee_size) as usize
            {
                // if the block is marked as confirmed (SHARDING NETWORK VERSIONS+ ONLY) there must be at least 2/3 of a comitee of signatures
                // TODO: We are now planning on using retroactive comitee size calculation. In short, the comitee size will change dependent on the
                // number of fullnodes (each epoch). Account for this and read the stored data of the epoch this block was in (or get it if its the current epoch)
                // we also need to account for delegate nodes which wont sign
                return Err(blockValidationErrors::tooLittleSignatures);
            } else {
                for signature in blk.clone().node_signatures {
                    // check each verifyer signature, a delegate node will not include a invalid verifyer signature so this should not happen without mallicious intervention
                    if !signature.valid() {
                        return Err(blockValidationErrors::badNodeSignature);
                    }
                }
            }

            let prev_blk = getBlock(&blk.header.chain_key, &blk.header.height - 1); // get the top block of the chain, this SHOULD be the block mentioned in prev block hash
            trace!(
                "Prev block: {:?} for chain {}",
                prev_blk,
                blk.header.chain_key
            );
            if blk.header.prev_hash != prev_blk.hash && blk.header.prev_hash != "".to_owned() {
                // the last block in this chain does not equal the previous hash of this block
                debug!(
                    "Expected prev hash to be: {}, got: {}. For block at height: {}",
                    prev_blk.hash, blk.header.prev_hash, blk.header.height
                );
                return Err(blockValidationErrors::invalidPreviousBlockhash);
            } else if let Err(_) = getAccount(&blk.header.chain_key) {
                // this account doesn't exist, the first block must be a genesis block
                if blk.header.height != 0 {
                    return Err(blockValidationErrors::other);
                }
            } else if !blk.validSignature() && blk.block_type != BlockType::Recieve {
                // recieve blocks are not formed by the reciecver and so the signature will be invalid
                return Err(blockValidationErrors::badSignature);
            } else if blk.header.timestamp - (config().transaction_timestamp_max_offset as u64)
                > (SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_millis() as u64)
            {
                // the block is too far in future
                debug!("Block: {} too far in futre. Our time: {}, block time: {}, block justifyed time: {}. Delta {}", blk.hash, (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis() as u64), blk.header.timestamp, blk.header.timestamp - (config().transaction_timestamp_max_offset as u64),
            blk.header.timestamp - (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
        .as_millis() as u64),);
                return Err(blockValidationErrors::timestampInvalid);
            } else if blk.header.height != 0
                && getBlockFromRaw(blk.header.prev_hash).header.timestamp > blk.header.timestamp
            {
                return Err(blockValidationErrors::timestampInvalid);
            }
            for txn in blk.txns {
                // check each txn in the block is valid
                if let Err(e) = txn.valid() {
                    return Err(blockValidationErrors::invalidTransaction(e));
                } /*else { removed as this will return prematurley and result in only the first txn being validated
                      return Ok(());
                  }*/
            }
            return Ok(()); // if you got here there are no issues
        }
    } else {
        return Err(blockValidationErrors::blockExists); // this block already exists
    }
}

//todo write commentaion/docs for tests
pub mod genesis;
#[cfg(test)]
mod tests {
    use crate::rand::Rng;
    use crate::*;
    use avrio_config::*;
    extern crate simple_logger;
    pub struct item {
        pub cont: String,
    }
    impl Hashable for item {
        fn bytes(&self) -> Vec<u8> {
            self.cont.as_bytes().to_vec()
        }
    }
    pub fn hash(subject: String) -> String {
        return item { cont: subject }.hash_item();
    }
    #[test]
    fn test_block() {
        simple_logger::init_with_level(log::Level::Info).unwrap();
        let mut i_t: u64 = 0;
        let mut rng = rand::thread_rng();
        let rngc = randc::SystemRandom::new();
        for _i in 0..=1000 {
            let mut block = Block::default();
            block.header.network = config().network_id;

            let pkcs8_bytes = signature::Ed25519KeyPair::generate_pkcs8(&rngc).unwrap();
            let key_pair = signature::Ed25519KeyPair::from_pkcs8(pkcs8_bytes.as_ref()).unwrap();
            let peer_public_key_bytes = key_pair.public_key().as_ref();
            while i_t < 10 {
                let mut txn = Transaction {
                    hash: String::from(""),
                    amount: rng.gen(),
                    extra: String::from(""),
                    flag: 'n',
                    sender_key: String::from(""),
                    receive_key: (hash(String::from(
                        "rc".to_owned() + &rng.gen::<u64>().to_string(),
                    ))),
                    access_key: String::from(""),
                    gas_price: rng.gen::<u16>() as u64,
                    max_gas: rng.gen::<u16>() as u64,
                    gas: rng.gen::<u16>() as u64,
                    nonce: rng.gen(),
                    signature: String::from(""),
                    timestamp: 0,
                    unlock_time: 0,
                };
                txn.sender_key = bs58::encode(peer_public_key_bytes).into_string();
                txn.hash();
                // Sign the hash
                let msg: &[u8] = txn.hash.as_bytes();
                txn.signature = bs58::encode(key_pair.sign(msg)).into_string();
                let _peer_public_key =
                    signature::UnparsedPublicKey::new(&signature::ED25519, peer_public_key_bytes);
                //peer_public_key.verify(msg, bs58::decode(&txn.signature.to_owned()).unwrap().as_ref()).unwrap();
                block.txns.push(txn);
                i_t += 1;
            }
            block.hash();
            let msg: &[u8] = block.hash.as_bytes();
            block.signature = bs58::encode(key_pair.sign(msg)).into_string();
            block.header.chain_key = bs58::encode(peer_public_key_bytes).into_string();
            println!("constructed block: {}, checking signature...", block.hash);
            assert_eq!(block.validSignature(), true);
            let block_clone = block.clone();
            println!("saving block");
            let conf = Config::default();
            let _ = conf.create();
            println!("Block: {:?}", block);
            saveBlock(block).unwrap();
            println!("reading block...");
            let block_read = getBlockFromRaw(block_clone.hash.clone());
            println!("read block: {}", block_read.hash);
            assert_eq!(block_read, block_clone);
        }
    }
}
