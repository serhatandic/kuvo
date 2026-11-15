//! Noise_IK_25519_AESGCM_SHA256, as sessions use it (PROTOCOL.md, "Sessions"), held to
//! `protocol/vectors/noise.json` as the Swift and TypeScript sides are. The initiator is this
//! client's side; the responder is here for the tests' stand-in host.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

pub const PROLOGUE: &[u8] = b"kuvo-link v1";
const IK: &[u8] = b"Noise_IK_25519_AESGCM_SHA256";

/// What went wrong reading a handshake or a transport message: never more than that it didn't open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed;

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any length is a key");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

fn hkdf(chaining: &[u8; 32], input: &[u8]) -> ([u8; 32], [u8; 32]) {
    let temp = hmac(chaining, &[input]);
    let first = hmac(&temp, &[&[1]]);
    let second = hmac(&temp, &[&first, &[2]]);
    (first, second)
}

fn public_of(private: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*private)).to_bytes()
}

fn dh(private: &[u8; 32], public: &[u8; 32]) -> [u8; 32] {
    StaticSecret::from(*private).diffie_hellman(&PublicKey::from(*public)).to_bytes()
}

/// One direction's key and count; with no key yet it passes bytes as they are.
struct Cipher {
    key: Option<[u8; 32]>,
    n: u64,
}

impl Cipher {
    fn nonce(&self) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&self.n.to_be_bytes());
        nonce
    }

    fn encrypt(&mut self, ad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let Some(key) = &self.key else { return plaintext.to_vec() };
        let sealed = Aes256Gcm::new(key.into()).encrypt(Nonce::from_slice(&self.nonce()), Payload { msg: plaintext, aad: ad }).expect("sealing a message of this size never fails");
        self.n += 1;
        sealed
    }

    fn decrypt(&mut self, ad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, Failed> {
        let Some(key) = &self.key else { return Ok(ciphertext.to_vec()) };
        let opened = Aes256Gcm::new(key.into()).decrypt(Nonce::from_slice(&self.nonce()), Payload { msg: ciphertext, aad: ad }).map_err(|_| Failed)?;
        self.n += 1;
        Ok(opened)
    }
}

struct Symmetric {
    h: [u8; 32],
    ck: [u8; 32],
    cipher: Cipher,
}

impl Symmetric {
    fn new(prologue: &[u8]) -> Symmetric {
        let mut h = [0u8; 32];
        if IK.len() <= 32 {
            h[..IK.len()].copy_from_slice(IK);
        } else {
            h = sha256(&[IK]);
        }
        let mut state = Symmetric { h, ck: h, cipher: Cipher { key: None, n: 0 } };
        state.mix_hash(prologue);
        state
    }

    fn mix_key(&mut self, input: &[u8]) {
        let (chaining, key) = hkdf(&self.ck, input);
        self.ck = chaining;
        self.cipher = Cipher { key: Some(key), n: 0 };
    }

    fn mix_hash(&mut self, data: &[u8]) {
        self.h = sha256(&[&self.h, data]);
    }

    fn encrypt_and_hash(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let h = self.h;
        let ciphertext = self.cipher.encrypt(&h, plaintext);
        self.mix_hash(&ciphertext);
        ciphertext
    }

    fn decrypt_and_hash(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, Failed> {
        let h = self.h;
        let plaintext = self.cipher.decrypt(&h, ciphertext)?;
        self.mix_hash(ciphertext);
        Ok(plaintext)
    }

    fn split(&self) -> (Cipher, Cipher) {
        let (first, second) = hkdf(&self.ck, &[]);
        (Cipher { key: Some(first), n: 0 }, Cipher { key: Some(second), n: 0 })
    }
}

/// A session's two directions, once its handshake is done.
pub struct Transport {
    pub hash: [u8; 32],
    sending: Cipher,
    receiving: Cipher,
}

impl Transport {
    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        self.sending.encrypt(&[], plaintext)
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, Failed> {
        self.receiving.decrypt(&[], ciphertext)
    }
}

/// The side that starts (this client), knowing the box's static key.
pub struct Initiator {
    s: [u8; 32],
    rs: [u8; 32],
    e: [u8; 32],
    state: Symmetric,
}

impl Initiator {
    pub fn new(s: [u8; 32], rs: [u8; 32], prologue: &[u8]) -> Initiator {
        let mut e = [0u8; 32];
        getrandom::fill(&mut e).expect("the system gives random bytes");
        Initiator::with_ephemeral(s, rs, prologue, e)
    }

    /// With the ephemeral key given (the vectors').
    pub fn with_ephemeral(s: [u8; 32], rs: [u8; 32], prologue: &[u8], e: [u8; 32]) -> Initiator {
        let mut state = Symmetric::new(prologue);
        state.mix_hash(&rs);
        Initiator { s, rs, e, state }
    }

    pub fn write_message_1(&mut self, payload: &[u8]) -> Vec<u8> {
        let ephemeral = public_of(&self.e);
        self.state.mix_hash(&ephemeral);
        self.state.mix_key(&dh(&self.e, &self.rs));
        let s = self.state.encrypt_and_hash(&public_of(&self.s));
        self.state.mix_key(&dh(&self.s, &self.rs));
        let payload = self.state.encrypt_and_hash(payload);
        [&ephemeral[..], &s, &payload].concat()
    }

    pub fn read_message_2(mut self, message: &[u8]) -> Result<(Vec<u8>, Transport), Failed> {
        if message.len() < 48 {
            return Err(Failed);
        }
        let re: [u8; 32] = message[..32].try_into().map_err(|_| Failed)?;
        self.state.mix_hash(&re);
        self.state.mix_key(&dh(&self.e, &re));
        self.state.mix_key(&dh(&self.s, &re));
        let payload = self.state.decrypt_and_hash(&message[32..])?;
        let (first, second) = self.state.split();
        Ok((payload, Transport { hash: self.state.h, sending: first, receiving: second }))
    }
}

/// The side that answers (a box), learning the client's static key. Used by the tests' stand-in.
pub struct Responder {
    s: [u8; 32],
    e: [u8; 32],
    state: Symmetric,
    re: [u8; 32],
    /// The client's static key, once its first message is read.
    pub rs: [u8; 32],
}

impl Responder {
    pub fn new(s: [u8; 32], prologue: &[u8]) -> Responder {
        let mut e = [0u8; 32];
        getrandom::fill(&mut e).expect("the system gives random bytes");
        Responder::with_ephemeral(s, prologue, e)
    }

    pub fn with_ephemeral(s: [u8; 32], prologue: &[u8], e: [u8; 32]) -> Responder {
        let mut state = Symmetric::new(prologue);
        state.mix_hash(&public_of(&s));
        Responder { s, e, state, re: [0; 32], rs: [0; 32] }
    }

    pub fn read_message_1(&mut self, message: &[u8]) -> Result<Vec<u8>, Failed> {
        if message.len() < 96 {
            return Err(Failed);
        }
        self.re = message[..32].try_into().map_err(|_| Failed)?;
        self.state.mix_hash(&self.re);
        self.state.mix_key(&dh(&self.s, &self.re));
        self.rs = self.state.decrypt_and_hash(&message[32..80])?.try_into().map_err(|_| Failed)?;
        self.state.mix_key(&dh(&self.s, &self.rs));
        self.state.decrypt_and_hash(&message[80..])
    }

    pub fn write_message_2(mut self, payload: &[u8]) -> (Vec<u8>, Transport) {
        let ephemeral = public_of(&self.e);
        self.state.mix_hash(&ephemeral);
        self.state.mix_key(&dh(&self.e, &self.re));
        self.state.mix_key(&dh(&self.e, &self.rs));
        let message = [&ephemeral[..], &self.state.encrypt_and_hash(payload)].concat();
        let (first, second) = self.state.split();
        (message, Transport { hash: self.state.h, sending: second, receiving: first })
    }
}
