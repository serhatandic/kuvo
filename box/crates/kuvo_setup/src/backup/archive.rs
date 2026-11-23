//! Version-one backup envelope. One GCM message covers the entire stream.
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use aes::{
    Aes256,
    cipher::{Block, BlockEncrypt, KeyInit, KeyIvInit, StreamCipher},
};
use ctr::Ctr32BE;
use ghash::{GHash, universal_hash::UniversalHash};
use kuvo_store::declarations::BACKUP_ARCHIVE_VERSION;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::{Error, RecoveryKey, Result};

const BLOCK: usize = 256 * 1024;
const MAX_HEADER: usize = 32 * 1024;
const MAX_PAYLOAD: u64 = (1 << 36) - 32;

#[derive(Debug)]
pub(super) enum Contents {
    Bytes(Vec<u8>),
    File {
        path: PathBuf,
        size: Option<u64>,
        digest: Option<String>,
    },
}

#[derive(Debug)]
pub(super) struct Entry {
    pub name: String,
    pub contents: Contents,
}

impl Entry {
    pub fn json(name: &str, value: &impl Serialize) -> Result<Self> {
        Ok(Self {
            name: name.into(),
            contents: Contents::Bytes(serde_json::to_vec(value)?),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    name: String,
    size: u64,
}

fn magic() -> String {
    format!("KUVO backup {BACKUP_ARCHIVE_VERSION}\n")
}

pub(super) fn valid_name(name: &str) -> bool {
    name.encode_utf16().count() <= 4096
        && !name.contains(['\\', '\0'])
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

pub(super) fn regular(path: &Path) -> Result<File> {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || !fs::symlink_metadata(path)?.is_file() {
        return Err(Error::Message("Backup needs regular files."));
    }
    Ok(file)
}

// The RustCrypto one-shot AEAD would require buffering the whole archive.
// Feed its constituent CTR and GHASH primitives incrementally. Only the last
// ciphertext block is padded. The AAD and length block follow SP 800-38D.
struct Gcm {
    cipher: Ctr32BE<Aes256>,
    hash: GHash,
    mask: Block<Aes256>,
    tail: [u8; 16],
    tail_len: usize,
    aad_len: u64,
    len: u64,
}

impl Gcm {
    fn new(key: &RecoveryKey, header: &[u8], context: &[u8]) -> Self {
        let aes = Aes256::new((&key.0).into());
        let mut h = Block::<Aes256>::default();
        aes.encrypt_block(&mut h);
        let mut hash = GHash::new(&h);
        let mut aad = header.to_vec();
        aad.extend_from_slice(context);
        hash.update_padded(&aad);
        let mut counter = Block::<Aes256>::default();
        counter[..12].copy_from_slice(&header[header.len() - 12..]);
        counter[15] = 1;
        let mut mask = counter;
        aes.encrypt_block(&mut mask);
        counter[15] = 2;
        Self {
            cipher: Ctr32BE::new((&key.0).into(), &counter),
            hash,
            mask,
            tail: [0; 16],
            tail_len: 0,
            aad_len: aad.len() as u64,
            len: 0,
        }
    }

    fn ciphertext(&mut self, mut bytes: &[u8]) {
        self.len += bytes.len() as u64;
        if self.tail_len != 0 {
            let take = bytes.len().min(16 - self.tail_len);
            self.tail[self.tail_len..self.tail_len + take].copy_from_slice(&bytes[..take]);
            self.tail_len += take;
            if self.tail_len < 16 {
                return;
            }
            self.hash.update(&[self.tail.into()]);
            bytes = &bytes[take..];
        }
        let whole = bytes.len() / 16 * 16;
        self.hash.update_padded(&bytes[..whole]);
        let tail = &bytes[whole..];
        self.tail[..tail.len()].copy_from_slice(tail);
        self.tail_len = tail.len();
    }

    fn tag(mut self) -> [u8; 16] {
        self.hash.update_padded(&self.tail[..self.tail_len]);
        let mut lengths = [0; 16];
        lengths[..8].copy_from_slice(&(self.aad_len * 8).to_be_bytes());
        lengths[8..].copy_from_slice(&(self.len * 8).to_be_bytes());
        self.hash.update(&[lengths.into()]);
        let mut tag = self.hash.finalize();
        for (byte, mask) in tag.iter_mut().zip(self.mask) {
            *byte ^= mask;
        }
        tag.into()
    }

    fn write(&mut self, output: &mut File, bytes: &mut [u8]) -> Result<()> {
        self.cipher.apply_keystream(bytes);
        self.ciphertext(bytes);
        Ok(output.write_all(bytes)?)
    }
}

pub(super) fn seal(
    output: &mut File,
    key: &RecoveryKey,
    context: &[u8],
    entries: impl IntoIterator<Item = Result<Entry>>,
) -> Result<()> {
    let mut header = magic().into_bytes();
    header.extend_from_slice(&super::random::<12>()?);
    let mut stream = Gcm::new(key, &header, context);
    output.write_all(&header)?;
    let mut block = vec![0; BLOCK];
    for entry in entries {
        let Entry { name, contents } = entry?;
        if !valid_name(&name) {
            return Err(Error::Message("Invalid backup entry name."));
        }
        let mut source = None;
        let size = match &contents {
            Contents::Bytes(bytes) => bytes.len() as u64,
            Contents::File { path, size, .. } => {
                let stamp = kuvo_store::files::file_stamp(path)?.ok_or(Error::Message(
                    "Kuvo data changed during backup. Try again.",
                ))?;
                if size.is_some_and(|size| size != stamp.size) {
                    return Err(Error::Detail(format!(
                        "Backup payload size differs from its record: {name}."
                    )));
                }
                let file = regular(path)?;
                let len = file.metadata()?.len();
                if stamp.size != len {
                    return Err(Error::Message(
                        "Kuvo data changed during backup. Try again.",
                    ));
                }
                source = Some((file, stamp));
                len
            }
        };
        let mut item = serde_json::to_vec(&Header {
            name: name.clone(),
            size,
        })?;
        if size > MAX_PAYLOAD - stream.len
            || 8 + item.len() as u64 > MAX_PAYLOAD - stream.len - size
        {
            return Err(Error::Message(
                "The backup exceeds this archive format’s size limit.",
            ));
        }
        stream.write(output, &mut (item.len() as u32).to_be_bytes())?;
        stream.write(output, &mut item)?;
        match (contents, source) {
            (Contents::Bytes(mut bytes), _) => stream.write(output, &mut bytes)?,
            (Contents::File { path, digest, .. }, Some((mut file, stamp))) => {
                let mut hash = digest.as_ref().map(|_| Sha256::new());
                let mut left = size;
                while left != 0 {
                    let len = left.min(BLOCK as u64) as usize;
                    let piece = &mut block[..len];
                    file.read_exact(piece)?;
                    if let Some(hash) = &mut hash {
                        hash.update(&*piece);
                    }
                    stream.write(output, piece)?;
                    left -= len as u64;
                }
                if !fs::symlink_metadata(&path)?.is_file()
                    || kuvo_store::files::file_stamp(&path)?.as_ref() != Some(&stamp)
                {
                    return Err(Error::Message(
                        "Kuvo data changed during backup. Try again.",
                    ));
                }
                if let Some((hash, expected)) = hash.zip(digest) {
                    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
                    if URL_SAFE_NO_PAD.encode(hash.finalize()) != expected {
                        return Err(Error::Detail(format!(
                            "Attachment changed during export: {}.",
                            name
                        )));
                    }
                }
            }
            _ => unreachable!("a file entry has an open descriptor"),
        }
    }
    stream.write(output, &mut [0; 4])?;
    output.write_all(&stream.tag())?;
    output.sync_all()?;
    Ok(())
}

pub(super) fn authenticate(
    file: &Path,
    key: &RecoveryKey,
    context: &[u8],
    mut plain: Option<&mut File>,
) -> Result<u64> {
    let mut input = regular(file)?;
    let magic = magic();
    let mut header = vec![0; magic.len() + 12];
    let len = input
        .metadata()?
        .len()
        .checked_sub((header.len() + 16) as u64)
        .filter(|len| (1..=MAX_PAYLOAD).contains(len))
        .ok_or(Error::Message("Incomplete backup."))?;
    input.read_exact(&mut header)?;
    if !header.starts_with(magic.as_bytes()) {
        return Err(Error::Message("Unsupported backup archive format."));
    }
    let mut stream = Gcm::new(key, &header, context);
    let mut block = vec![0; BLOCK];
    let mut left = len;
    while left != 0 {
        let size = left.min(BLOCK as u64) as usize;
        let piece = &mut block[..size];
        input.read_exact(piece)?;
        stream.ciphertext(piece);
        if let Some(output) = &mut plain {
            stream.cipher.apply_keystream(piece);
            output.write_all(piece)?;
        }
        left -= size as u64;
    }
    let mut tag = [0; 16];
    input.read_exact(&mut tag)?;
    if !bool::from(stream.tag().ct_eq(&tag)) {
        return Err(Error::Message(
            "Backup verification failed. Check the recovery key and archive.",
        ));
    }
    Ok(len)
}

pub(super) struct Opened {
    pub name: String,
    pub file: PathBuf,
}

fn with_entries<T>(
    file: &Path,
    key: &RecoveryKey,
    context: &[u8],
    scratch: &Path,
    mut read: impl FnMut(String, &mut io::Take<&mut File>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut plain = tempfile::tempfile_in(scratch)?;
    let mut left = authenticate(file, key, context, Some(&mut plain))?;
    plain.seek(SeekFrom::Start(0))?;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    loop {
        if left < 4 {
            return Err(Error::Message("Invalid backup header."));
        }
        let mut prefix = [0; 4];
        plain.read_exact(&mut prefix)?;
        left -= 4;
        let length = u32::from_be_bytes(prefix) as usize;
        if length == 0 && left == 0 {
            return Ok(entries);
        }
        if length == 0 || length > MAX_HEADER || length as u64 > left {
            return Err(Error::Message("Invalid backup header."));
        }
        let mut header = vec![0; length];
        plain.read_exact(&mut header)?;
        left -= length as u64;
        let Header { name, size } = serde_json::from_slice(&header)?;
        if !valid_name(&name) || !seen.insert(name.clone()) || left < 4 || size > left - 4 {
            return Err(Error::Message("Invalid backup entry."));
        }
        let mut contents = (&mut plain).take(size);
        entries.push(read(name, &mut contents)?);
        if contents.limit() != 0 {
            return Err(Error::Message("Backup data ended unexpectedly."));
        }
        left -= size;
    }
}

pub(super) fn open(
    file: &Path,
    key: &RecoveryKey,
    context: &[u8],
    scratch: &Path,
) -> Result<Vec<Opened>> {
    let mut number = 0;
    with_entries(file, key, context, scratch, |name, contents| {
        let file = scratch.join(number.to_string());
        number += 1;
        let mut output = tempfile::NamedTempFile::new_in(scratch)?;
        io::copy(contents, output.as_file_mut())?;
        output
            .persist_noclobber(&file)
            .map_err(|error| error.error)?;
        Ok(Opened { name, file })
    })
}

pub(super) fn single_json<T: serde::de::DeserializeOwned>(
    file: &Path,
    key: &RecoveryKey,
    context: &[u8],
    scratch: &Path,
    expected: &str,
) -> Result<T> {
    let mut read = false;
    with_entries(file, key, context, scratch, |name, contents| {
        if name != expected || read {
            return Err(Error::Message("Unexpected entry in the backup."));
        }
        read = true;
        Ok(serde_json::from_reader(contents)?)
    })?
    .pop()
    .ok_or(Error::Message("The backup entry is missing."))
}
