use crate::{
    format::{P2pData, LEN_DECL_BYTES},
    peer::{strip_port, PEERS},
};
use rust_crypto::{
    aead::{AeadDecryptor, AeadEncryptor},
    aes::KeySize,
    aes_gcm::AesGcm,
};
use std::error::Error;
use std::io::{Read, Write};
use std::net::TcpStream;
use log::trace;

pub fn peek(peer: &mut TcpStream) -> Result<usize, std::io::Error> {
    let mut buf = [0; 1000000];
    return peer.peek(&mut buf);
}

pub trait Sendable {
    /// # encode
    /// Should encode T into a String that can be transported and then decoded with the decode function
    fn encode(&self) -> Result<String, Box<dyn Error>>;
    /// # decode
    /// Should take a reference to a String and return T
    fn decode(s: &String) -> Result<Box<Self>, Box<dyn Error>>;
    /// # send_raw
    /// Encodes T into a string and sends it to the peer
    /// # Params
    /// Takes a reference to self, a mutable refrence to a TcpStream and a bool value. The bool value the function if it should flush the stream after use.
    /// If you want your data to get there promptly and it is small you should use this. In low profile you should not
    ///
    /// # Panics
    /// Does not panic but can return a Error
    ///
    /// # Blocking
    /// This function will not block and makes no assumptions about the sate of the peer;
    /// Just because this returns Ok(()) does not mean the peer is connected or has seen the message
    fn send_raw(&self, peer: &mut TcpStream, flush: bool) -> Result<(), Box<dyn Error>> {
        let en = self.encode()?;
        let buf = en.as_bytes();
        peer.write(&buf)?;
        if flush {
            peer.flush()?;
        }
        return Ok(());
    }
}
/// A struct to allow you to easly use strings with the Sendable trait
struct S {
    pub c: String,
}

impl Sendable for S {
    fn encode(&self) -> Result<String, Box<dyn Error>> {
        return Ok(self.c.clone());
    }
    fn decode(s: &String) -> Result<Box<Self>, Box<dyn Error>> {
        return Ok(S { c: s.clone() }.into());
    }
}

impl S {
    pub fn from_string(s: String) -> Self {
        S { c: s }
    }
    pub fn to_string(self) -> String {
        self.c
    }
}

/// # Send
/// This function will form a P2pData Struct from your message and message type, encode it into a string and send it to the specifyed stream
/// It is a convienience wrapper wrong the sen function from the Sendable trait
/// # Params
/// Takes a mutable refrence to a TcpStream, a bool value and a Option<slice>. The bool value the function if it should flush the stream after use.
/// If you want your data to get there promptly and it is small you should use this. In low profile you should not.
/// The option<slice> is the key, if not passed it will attempt to get it from the PEERS global variable. If that fails it will return an Error
///
///  # Panics
/// Does not panic but can return a Error
///
/// # Blocking
/// This function will not block and makes no assumptions about the sate of the peer;
/// Just because this returns Ok(()) does not mean the peer is connected or has seen the message
pub fn send(
    msg: String,
    peer: &mut TcpStream,
    msg_type: u16,
    flush: bool,
    key: Option<&[u8]>,
) -> Result<(), Box<dyn Error>> {
    let p2p_dat = P2pData::gen(msg, msg_type);
    p2p_dat.log();
    let data: String = p2p_dat.to_string();
    let s: S;
    let mut k: AesGcm;
    if key.is_some() {
        let key_unwraped = key.unwrap();
        trace!("KEY: {:?}, LEN: {}", key_unwraped, key_unwraped.len());
        let mut k = AesGcm::new(
            KeySize::KeySize128,
            key_unwraped,
            &[0; 12],
            p2p_dat.length().to_string().as_bytes(),
        );
        let mut tag = vec![];
        let mut output = vec![];
        let _ = k.encrypt(data.as_bytes(), &mut output, &mut tag);
        s = S {
            c: format!("{}@{}", hex::encode(output), hex::encode(tag)),
        };
    } else {
        let map = PEERS.lock()?;
        if let Some(val) = map.get(&strip_port(&peer.peer_addr()?)) {
            k = AesGcm::new(
                KeySize::KeySize128,
                val.0.as_bytes(),
                &[0],
                p2p_dat.length().to_string().as_bytes(),
            );
            let mut tag = vec![];
            let mut output = vec![];
            let _ = k.encrypt(data.as_bytes(), &mut output, &mut tag);
            s = S {
                c: format!("{}@{}", hex::encode(output), hex::encode(tag)),
            };
        } else {
            return Err("No key provided and peer not found".into());
        }
    }
    return s.send_raw(peer, flush);
}

/// # Read
/// Reads data from the specifyed stream and parses it into a P2pData struct.
///
/// # Params
/// Takes a mutable refrence to a TcpStream, a Option<u64> value and a Option<slice> value. The Option value is the timeout.
/// If set it tells the function long to wait before returning if we dont get any data. If set to None it will block
/// infinitly or untll data is got. The Option<Slice> value is they encryption key. If None it will get from the PEERS global value
///
/// # Panics
/// Does not panic but can return a Error
///
/// # Blocking
/// This function will block until either:
/// * The time since start exceeds the timeout value (if it is set)
/// * It reads the data
pub fn read(
    peer: &mut TcpStream,
    timeout: Option<u64>,
    key: Option<&[u8]>,
) -> Result<P2pData, Box<dyn Error>> {
    let start = std::time::SystemTime::now();
    loop {
        if timeout.is_some() {
            if std::time::SystemTime::now()
                .duration_since(start)?
                .as_millis() as u64
                > timeout.unwrap()
            {
                return Err("Timed out".into());
            }
        }
        let mut buf = [0; LEN_DECL_BYTES];
        if let Ok(a) = peer.peek(&mut buf) {
            if a != 0 {
            } else {
                // read exactly 8 bytes whixh is the len of the message
                peer.read_exact(&mut buf)?;
                // convert the bytes into a string
                let len_s = String::from_utf8(buf.to_vec())?;
                let len_striped: String = len_s.trim_start_matches("0").to_string();
                let len: usize = len_striped.parse()?;
                let mut k: AesGcm;
                if key.is_some() {
                    k = AesGcm::new(
                        KeySize::KeySize128,
                        key.unwrap(),
                        &[0; 12],
                        len.to_string().as_bytes(),
                    );
                } else {
                    let map = PEERS.lock()?;
                    if let Some(val) = map.get(&strip_port(&peer.peer_addr()?)) {
                        k = AesGcm::new(
                            KeySize::KeySize128,
                            val.0.as_bytes(),
                            &[0; 12],
                            len.to_string().as_bytes(),
                        );
                    } else {
                        return Err("No key provided and peer not found".into());
                    }
                }
                let mut buf = Vec::with_capacity(len);
                peer.read_exact(&mut buf)?;
                let s: String = String::from_utf8(buf.to_vec())?
                    .trim_matches('0')
                    .to_string();
                let s_split: Vec<&str> = s.split("@").collect();
                if s_split.len() < 1 {
                    return Err("No auth tag".into());
                } else {
                    let cf: &[u8] = &hex::decode(s_split[0])?;
                    let tag: &[u8] = &hex::decode(s_split[1])?;
                    let mut out = vec![];
                    if !k.decrypt(cf, &mut out, tag) {
                        return Err("failed to decrypt message".into());
                    } else {
                        return Ok(P2pData::from_string(&format!(
                            "{}{}{}",
                            len_s,
                            String::from_utf8(out)?,
                            len_s
                        )));
                    }
                }
            }
        }
    }
}
