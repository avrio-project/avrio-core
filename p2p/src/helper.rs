use crate::{
    format::P2pData,
    io::{read, send},
    peer::{get_peers_addr, lock, locked, unlock_peer},
    utils::*,
};
use avrio_blockchain::{
    check_block, enact_block, enact_send, generate_merkle_root_all, getBlock, getBlockFromRaw,
    saveBlock, Block, BlockType,
};
use avrio_config::config;
use avrio_database::{get_data, save_data};

//use bson;
use log::*;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::net::TcpStream;
use std::thread;

/// TODO: implment this
pub fn get_peerlist_from_peer(
    _peer: &mut TcpStream,
) -> Result<Vec<std::net::SocketAddr>, Box<dyn Error>> {
    return Ok(vec![]);
}

pub fn sync_needed() -> Result<bool, Box<dyn Error>> {
    let mut chain_digests: Vec<String> = vec![];
    for peer in get_peers_addr().unwrap_or_default() {
        // ask every connected peer for their chain digest
        trace!("Getting chain digest for peer: {:?}", peer);
        let mut peer_stream = lock(&peer, 1000)?;
        chain_digests.push(get_chain_digest_string(&mut peer_stream, true));
        unlock_peer(peer_stream)?;
    }
    if chain_digests.len() == 0 {
        // if we get no chain digests
        trace!("Got not chain digests");
        return Ok(true); // should we not return an error or at least false?
    } else {
        // we got at least one chain digest
        // find the most common chain digest
        let mode: String = get_mode(chain_digests.clone());
        let ours = get_data(config().db_path + &"/chaindigest".to_owned(), &"master");
        debug!(
            "Chain digests: {:#?}, mode: {}, ours: {}",
            chain_digests, mode, ours
        );
        if ours == mode {
            // we have the most common chain digest, we are 'up-to date'
            return Ok(false);
        } else {
            //  we are not on the most common chain digest, sync with any peers with that digest
            return Ok(true);
        }
    }
}

/// # Prop_block
/// Sends a block to all connected peers.
/// # Returns
/// a result enum conatining the error encountered or a u64 of the number of peers we sent to and got a block ack response from
/// Once proof of node is in place it will send it only to the relevant comitee.
pub fn prop_block(blk: &Block) -> Result<u64, Box<dyn std::error::Error>> {
    let mut i: u64 = 0;
    for peer in get_peers_addr()?.iter_mut() {
        debug!("Sending block to peer: {:?}", peer);
        let mut peer_stream = lock(peer, 1000)?;
        let send_res = send_block_struct(blk, &mut peer_stream);
        if let Ok(_) = send_res {
            i += 1;
        } else {
            trace!("error sending block to peer {}, error={}", peer_stream.peer_addr()?, send_res.unwrap_err());
        }
        let _ = unlock_peer(peer_stream)?;
    }
    trace!("Sent block {} to {} peers", blk.hash, i);
    return Ok(i);
}

/// This is a cover all sync function that will sync all chains and covers getting the top index and syncing from there
/// for more controll over your sync you should call the sync_chain function which will sync only the chain specifyed.
/// pl is a vector of mutable refrences of TcpStreams (Vec<&mut TcpStream>), this function finds the most common chain digest
/// and then chooses the fasted peer with that chain digest and uses it. After it thinks it has finished syncing it will choose
/// a random peer and check random blocks are the same. If you wish to use the sync function with only one peer pass a vector
/// containing only that peer. Please note this means it will not be able to verify that it has not missed blocks afterwards if
/// the peer is malicously withholding them. For this reason only do this if you trust the peer or will be checking the blockchain
/// with a diferent peer afterwards.
pub fn sync() -> Result<u64, String> {
    let mut pl = get_peers_addr().unwrap_or_default(); // list of all socket addrs
    std::thread::sleep(std::time::Duration::from_millis(500)); // wait 0.5 (500ms) seccond to ensure handler thread is paused
    if pl.len() < 1 {
        return Err("Must have at least one peer to sync from".into());
    }

    let mut _peers: Vec<TcpStream> = vec![];
    let _pc: u32 = 0;
    let _i: usize = 0;
    let mut chain_digests: Vec<ChainDigestPeer> = vec![];

    for peer in pl.iter_mut() {
        //let _ = lock_peer(&peer.peer_addr().unwrap().to_string()).unwrap();

        if let Ok(mut peer_new) = lock(peer, 1000) {
            let mut cloned_peer = peer_new.try_clone().unwrap();
            _peers.push(peer_new);
            let handle = thread::Builder::new()
                .name("getChainDigest".to_string())
                .spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1000)); // wait 350ms for the handler thread to see our message and stop. TODO: wait for a response from the thread instead
                    log::trace!("Get chain digest waited 100ms, proceeding");
                    let chain_digest = get_chain_digest(&mut cloned_peer, false);

                    if chain_digest.digest == " " {
                        return ChainDigestPeer {
                            peer: Some(cloned_peer),
                            digest: " ".to_string(),
                        };
                    } else {
                        return chain_digest;
                    }
                });
            if let Ok(handle_) = handle {
                if let Ok(result) = handle_.join() {
                    chain_digests.push(result);
                }
            }
        }
    }

    let mut hashes: Vec<String> = vec![];
    // let chainDigestsLen = chain_digests.len();

    for hash in chain_digests.iter() {
        hashes.push(hash.digest.clone());
    }

    let mode_hash = get_mode(hashes);
    let mut peer_to_use: Option<TcpStream> = None;
    let _i: u64 = 0;

    for i in 0..chain_digests.len() {
        if chain_digests[i].digest == mode_hash {
            if let Some(peer_) = &chain_digests[i].peer {
                peer_to_use = Some(peer_.try_clone().unwrap());
            }
        }
    }
    drop(chain_digests);
    let mut peer_to_use_unwraped: TcpStream = peer_to_use.unwrap();

    // Now unlock all peers we are not going to be using
    let peer_to_use_addr = peer_to_use_unwraped.peer_addr().unwrap();
    for peer in _peers.iter_mut() {
        if &peer.peer_addr().unwrap() != &peer_to_use_addr {
            // Clone the peer var to get a Stream object (rather than a mutable refrence), pass that to unlock_peer then
            // after this loop drop the _peers list to destroy all the og streams
            unlock_peer(peer.try_clone().unwrap());
        }
    }
    drop(_peers); // destroy all remaining streams

    let try_ack = syncack_peer(&mut peer_to_use_unwraped, false);
    if let Err(e) = try_ack {
        error!("Got error: {} when sync acking peer. Releasing lock", e);
        unlock_peer(peer_to_use_unwraped);
        // TODO sync ack the next fastest peer until we have peer (1)
        return Err("rejected sync ack".into());
    } else {
        // Relock peer
        //lock_peer(&peer_to_use_unwraped.peer_addr().unwrap().to_string()).unwrap();

        // We have locked the peer now we ask them for their list of chains
        // They send their list of chains as a vec of strings
        if let Err(e) = send("".to_owned(), &mut peer_to_use_unwraped, 0x60, true, None) {
            error!("Failed to request chains list from peer gave error: {}", e);
            // TODO: *1
            return Err("failed to send get chain list message".into());
        } else {
            let mut buf = [0; 10024];
            let mut no_read = true;

            while no_read == true {
                if let Ok(a) = peer_to_use_unwraped.peek(&mut buf) {
                    if a == 0 {
                    } else {
                        no_read = false;
                    }
                }
            }

            // There are now bytes waiting in the stream
            let deformed = read(&mut peer_to_use_unwraped, Some(10000), None).unwrap_or_default();
            debug!("Chain list got: {:#?}", deformed);

            if deformed.message_type != 0x61 {
                error!(
                    "Failed to get chain list from peer (got wrong message type back: {})",
                    deformed.message_type
                );

                //TODO: *1
                return Err("got wrong message response (context get chain list)".into());
            } else {
                let chain_list: Vec<String> =
                    serde_json::from_str(&deformed.message).unwrap_or_default();

                if chain_list.len() == 0 {
                    return Err("empty chain list".into());
                } else {
                    for chain in chain_list.iter() {
                        info!("Starting to sync chain: {}", chain);

                        if let Err(e) = sync_chain(chain.to_owned(), &mut peer_to_use_unwraped) {
                            error!("Failed to sync chain: {}, gave error: {}", chain, e);
                            return Err(format!("failed to sync chain {}", chain));
                        } else {
                            info!("Synced chain {}, moving onto next chain", chain);
                        }
                    }
                }
            }
        }
    }

    info!("Synced all chains, checking chain digest with peers");

    if get_data(config().db_path + &"/chaindigest", "master") != mode_hash {
        error!("Synced blocks do not result in mode block hash, if you have appended blocks (using send_txn or generate etc) then ignore this. If not please delete your data dir and resync");
        error!(
            "Our CD: {}, expected: {}",
            get_data(config().db_path + &"/chaindigest", "master"),
            mode_hash
        );

        return sync();
    } else {
        info!("Finalised syncing, releasing lock on peer");
        unlock_peer(peer_to_use_unwraped);
    }

    return Ok(1);
}

/// This function syncs the specifyed chain only from the peer specifyed.
/// It returns Ok(()) on succsess and handles the inventory generation, inventory saving, block geting, block validation,
/// block saving, block enacting and informing the user of the progress.
/// If you simply want to sync all chains then use the sync function bellow.
pub fn sync_chain(chain: String, peer: &mut TcpStream) -> Result<u64, Box<dyn std::error::Error>> {
    let _ = send(
        chain.to_owned(),
        &mut peer.try_clone().unwrap(),
        0x45,
        true,
        None,
    );
    let mut buf = [0; 1024];
    let mut no_read = true;

    while no_read == true {
        if let Ok(a) = peer.try_clone().unwrap().peek(&mut buf) {
            if a == 0 {
            } else {
                no_read = false;
            }
        }
    }

    // There are now bytes waiting in the stream
    let deformed: P2pData = read(peer, Some(10000), None).unwrap_or_else(|e| {
        error!("Failed to read p2pdata: {}", e);
        P2pData::default()
    });

    let amount_to_sync: u64;

    if deformed.message_type != 0x46 {
        warn!("Got wrong block count message from peer, this could cause syncing issues!");
        amount_to_sync = 0;
    } else {
        amount_to_sync = deformed.message.parse().unwrap_or_else(|e| {
            warn!("Failed to parse block count msg, gave error: {}", e);
            0
        });
    }

    let print_synced_every: u64;
    info!("Got to get {} blocks for chain: {}", amount_to_sync, chain);

    match amount_to_sync {
        0..=9 => print_synced_every = 1,
        10..=100 => print_synced_every = 10,
        101..=500 => print_synced_every = 50,
        501..=1000 => print_synced_every = 100,
        1001..=10000 => print_synced_every = 500,
        10001..=50000 => print_synced_every = 2000,
        _ => print_synced_every = 5000,
    }

    let top_block_hash: String;
    // let opened_db: rocksdb::DB;
    top_block_hash = get_data(
        config().db_path + "/chains/" + &chain + &"-chainindex",
        "topblockhash",
    );

    if top_block_hash == "-1" {
        if let Err(e) = send(
            serde_json::to_string(&(&"0".to_owned(), &chain))?,
            peer,
            0x6f,
            true,
            None,
        ) {
            error!(
                "Asking peer for their blocks above hash: {} for chain: {} gave error: {}",
                top_block_hash, chain, e
            );
            return Err(e.into());
        }
    } else if let Err(e) = send(
        serde_json::to_string(&(&top_block_hash, &chain))?,
        peer,
        0x6f,
        true,
        None,
    ) {
        error!(
            "Asking peer for their blocks above hash: {} for chain: {} gave error: {}",
            top_block_hash, chain, e
        );
        return Err(e.into());
    }

    let mut synced_blocks: u64 = 0;
    let mut invalid_blocks: u64 = 0;

    info!(
        "Getting {} blocks from peer: {}, from block hash: {}",
        amount_to_sync,
        peer.peer_addr().unwrap(),
        top_block_hash
    );

    loop {
        let mut no_read: bool = true;

        while no_read == true {
            if let Ok(a) = peer.peek(&mut buf) {
                if a == 0 {
                } else {
                    no_read = false;
                }
            }
        }

        // There are now bytes waiting in the stream
        let deformed: P2pData = read(peer, Some(10000), None).unwrap_or_else(|e| {
            error!("Failed to read p2pdata: {}", e);
            P2pData::default()
        });

        trace!(target: "avrio_p2p::sync", "got blocks: {:#?}", deformed);

        if deformed.message_type != 0x0a {
            // TODO: Ask for block(s) again rather than returning err
            error!(
                "Failed to get block, wrong message type: {}",
                deformed.message_type
            );
            return Err("failed to get block".into());
        } else {
            let blocks: Vec<Block> = serde_json::from_str(&deformed.message).unwrap_or_default();

            if blocks.len() != 0 {
                trace!(
                    "Got: {} blocks from peer. Hash: {} up to: {}",
                    blocks.len(),
                    blocks[0].hash,
                    blocks[blocks.len() - 1].hash
                );

                for block in blocks {
                    if let Err(e) = check_block(block.clone()) {
                        error!("Recieved invalid block with hash: {} from peer, validation gave error: {:#?}. Invalid blocks from peer: {}", block.hash, e, invalid_blocks);
                        invalid_blocks += 1;
                    } else {
                        saveBlock(block.clone())?;
                        if block.block_type == BlockType::Send {
                            enact_send(block)?;
                        } else {
                            enact_block(block)?;
                        }
                        synced_blocks += 1;
                    }
                    if synced_blocks % print_synced_every == 0 {
                        info!(
                            "Synced {} / {} blocks (chain: {}). {} more to go",
                            synced_blocks,
                            amount_to_sync,
                            chain,
                            amount_to_sync - synced_blocks
                        );
                    }
                }
            } else {
                synced_blocks = synced_blocks;
            }
        }

        if synced_blocks >= synced_blocks {
            info!("Synced all {} blocks for chain: {}", synced_blocks, chain);
            break;
        }

        let top_block_hash: String;

        top_block_hash = get_data(
            config().db_path + "/chains/" + &chain + &"-chainindex",
            "topblockhash",
        );

        trace!("Asking peer for blocks above hash: {}", top_block_hash);

        if top_block_hash == "-1" {
            if let Err(e) = send(
                serde_json::to_string(&(&"0", &chain))?,
                peer,
                0x6f,
                true,
                None,
            ) {
                error!(
                    "Asking peer for their blocks above hash: {} for chain: {} gave error: {}",
                    top_block_hash, chain, e
                );
                return Err(e.into());
            }
        } else if let Err(e) = send(
            serde_json::to_string(&(&top_block_hash, &chain))?,
            peer,
            0x6f,
            true,
            None,
        ) {
            error!(
                "Asking peer for their blocks above hash: {} for chain: {} gave error: {}",
                top_block_hash, chain, e
            );
            return Err(e.into());
        }
    }

    return Ok(amount_to_sync);
}

pub fn send_block_struct(block: &Block, peer: &mut TcpStream) -> Result<(), Box<dyn Error>> {
    if block.hash == Block::default().hash {
        return Err("tried to send default block".into());
    } else {
        let block_ser: String = serde_json::to_string(block)?; // serilise the block into bson

        if let Err(e) = send(block_ser, peer, 0x0a, true, None) {
            return Err(e.into());
        } else {
            return Ok(());
        }
    }
}

pub fn send_block_with_hash(hash: String, peer: &mut TcpStream) -> Result<(), Box<dyn Error>> {
    let block = avrio_blockchain::getBlockFromRaw(hash);
    if block.hash == Block::default().hash {
        return Err("block does not exist".into());
    } else {
        let block_ser: String = bson::to_bson(&block)?.to_string();

        if let Err(e) = send(block_ser, peer, 0x0a, true, None) {
            return Err(e.into());
        } else {
            return Ok(());
        }
    }
}

// -- Sync assist functions and structures-- //
// these should all be private, DO NOT PUBLICIZE THEM //

/// This function asks the peer to sync, if they accept you can begin syncing
pub fn syncack_peer(peer: &mut TcpStream, unlock: bool) -> Result<TcpStream, Box<dyn Error>> {
    //lock(&peer.peer_addr().unwrap(), 10000);

    let syncreqres = send(
        "syncreq".to_owned(),
        &mut peer.try_clone().unwrap(),
        0x22,
        true,
        None,
    );

    match syncreqres {
        Ok(()) => {}
        Err(e) => {
            error!("Failed to end syncreq message to peer, gave error: {}. Check your internet connection and ensure port is not in use!", e);
            return Err("failed to send syncreq".into());
        }
    };

    let mut buf = [0; 1024];
    let mut no_read = true;

    while no_read == true {
        if let Ok(a) = peer.peek(&mut buf) {
            if a == 0 {
            } else {
                no_read = false;
            }
        }
    }

    // There are now bytes waiting in the stream
    let deformed: P2pData = read(peer, Some(10000), None).unwrap_or_default();

    if unlock == true {
        debug!("Releasing lock on peer");
        //unlock_peer(peer).unwrap();
    }

    if deformed.message == "syncack".to_string() {
        info!("Got syncack from selected peer. Continuing");

        return Ok(peer.try_clone()?);
    } else if deformed.message == "syncdec".to_string() {
        info!("Peer rejected sync request, choosing new peer...");

        // choose the next fasted peer from our socket list
        return Err("rejected syncack".into());
    } else {
        info!("Recieved incorect message from peer (in context syncrequest). Message: {}. This could be caused by outdated software - check you are up to date!", deformed.message);
        info!("Retrying syncack with same peer...");

        // try again
        return syncack_peer(&mut peer.try_clone()?, unlock);
    }
}

/// Sends our chain digest, this is a merkle root of all the blocks we have.avrio_blockchain.avrio_blockchain
/// it is calculated with the generateChainDigest function which is auto called every time we get a new block
fn send_chain_digest(peer: &mut TcpStream) {
    let chains_digest = get_data(config().db_path + &"/chaindigest".to_owned(), &"master");

    trace!("sending our chain digest: {}", chains_digest);

    if chains_digest == "-1".to_owned() || chains_digest == "0".to_owned() {
        let _ = send(
            generate_merkle_root_all().unwrap_or("".to_owned()),
            peer,
            0x01,
            true,
            None,
        );
    } else {
        let _ = send(chains_digest, peer, 0x01, true, None);
    }
}

fn get_chain_digest_string(peer: &mut TcpStream, unlock: bool) -> String {
    let _ = send("".to_owned(), peer, 0x1c, true, None);
    let res = loop {
        let read = read(peer, Some(10000), None).unwrap_or_else(|e| {
            error!("Failed to read p2pdata: {}", e);
            P2pData::default()
        });

        break read.message;
    };

    return res;
}

/// this asks the peer for their chain digest
fn get_chain_digest(peer: &mut TcpStream, unlock: bool) -> ChainDigestPeer {
    while !locked(&peer.peer_addr().unwrap()).unwrap() {
        log::trace!("NOT LOCKED GCD");
    }
    let _ = send("".to_owned(), peer, 0x1c, true, None);

    let res = loop {
        let read = read(peer, Some(10000), None).unwrap_or_else(|e| {
            error!("Failed to read p2pdata: {}", e);
            P2pData::default()
        });

        let peer_n = peer.try_clone();

        if let Ok(peer_new) = peer_n {
            break ChainDigestPeer {
                peer: Some(peer_new),
                digest: read.message,
            };
        } else {
            break ChainDigestPeer {
                digest: "".to_string(),
                peer: None,
            };
        }
    };

    return res;
}

/// Struct for easily encoding data needed for the sorting of chain digests (used for choosing which peer/s to sync from)
#[derive(Debug, Default)]
pub struct ChainDigestPeer {
    pub peer: Option<TcpStream>,
    pub digest: String,
}

/// Struct for easily encoding data needed when askign for inventories
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct GetInventories {
    /// The amount of inventories to get back, max is 128
    /// if this value is 0 it uses the from and to hashes instead
    pub amount: u8,
    /// hash (or 00000000000 for ignore)
    /// if this value is 00000000000 it uses the first block
    pub from: String,
    /// hash (or 00000000000 for ignore)
    /// if this value is 00000000000 it will take the block that is *amount* blocks ahead of from and use that
    pub to: String,
}

/// Struct for easily encoding data needed for asking for blocks
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct GetBlocks {
    /// The hash of the block you want to get
    pub hash: String,
}

// -- End ync assist functions and structures-- //
