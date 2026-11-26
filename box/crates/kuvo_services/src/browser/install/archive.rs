//! Only files the browser needs, unpacked into its private staging folder.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
const MAX_ZIP: u64 = 2 * 1024 * 1024 * 1024;
const REGULAR: u32 = 0o100000;
const LINK: u32 = 0o120000;
const DIRECTORY: u32 = 0o040000;
const TYPE: u32 = 0o170000;

fn bad(why: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("The browser's download can't be unpacked ({why})."),
    )
}
fn parts(name: &str) -> io::Result<PathBuf> {
    let name = name.strip_suffix('/').unwrap_or(name);
    if name.is_empty()
        || name.contains(['\0', '\\'])
        || name
            .split('/')
            .any(|p| p.is_empty() || matches!(p, "." | ".."))
    {
        return Err(bad("a name outside the folder"));
    }
    let path = PathBuf::from(name);
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(bad("a name outside the folder"));
    }
    Ok(path)
}
fn folder(root: &Path, name: &Path) -> io::Result<PathBuf> {
    let mut target = root.to_owned();
    for part in name.components() {
        if !matches!(part, Component::Normal(_)) {
            return Err(bad("a name outside the folder"));
        }
        target.push(part);
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(bad("a name goes through something that isn't a folder")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&target)?,
            Err(error) => return Err(error),
        }
    }
    Ok(target)
}
#[cfg(unix)]
fn mode(file: &File, value: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(value))
}
#[cfg(not(unix))]
fn mode(_file: &File, _value: u32) -> io::Result<()> {
    Ok(())
}
#[cfg(unix)]
pub(super) fn link(target: &Path, path: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}
#[cfg(not(unix))]
pub(super) fn link(_target: &Path, _path: &Path) -> io::Result<()> {
    Err(io::Error::other("The browser runs on Linux and macOS."))
}
fn target(root: &Path, name: &Path) -> io::Result<PathBuf> {
    Ok(folder(root, name.parent().ok_or_else(|| bad("a name"))?)?
        .join(name.file_name().ok_or_else(|| bad("a name"))?))
}
fn create(name: &Path, mode_bits: u32) -> io::Result<File> {
    let file = OpenOptions::new().create_new(true).write(true).open(name)?;
    mode(&file, mode_bits)?;
    Ok(file)
}
fn check_link(name: &Path, destination: &Path) -> io::Result<()> {
    if destination.as_os_str().is_empty() || destination.to_string_lossy().contains(['\0', '\\']) {
        return Err(bad("a link points outside it"));
    }
    let mut depth = name.parent().map_or(0, |p| p.components().count());
    for part in destination.components() {
        match part {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return Err(bad("a link points outside it")),
        }
    }
    Ok(())
}
pub(super) fn links_inside(root: &Path) -> io::Result<()> {
    fn walk(root: &Path, folder: &Path) -> io::Result<()> {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let path = entry.path();
            if ty.is_symlink() {
                if !path
                    .canonicalize()
                    .is_ok_and(|p| p.starts_with(root) && p != root)
                {
                    return Err(bad("a link leads outside the build"));
                }
            } else if ty.is_dir() {
                walk(root, &path)?;
            }
        }
        Ok(())
    }
    walk(&root.canonicalize()?, root)
}
pub(super) fn unzip(file: &Path, root: &Path) -> io::Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(file)?).map_err(bad)?;
    if zip.len() > 200_000 {
        return Err(bad("its directory"));
    }
    let mut links = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut item = zip.by_index(i).map_err(bad)?;
        let name = parts(item.name())?;
        let mode_bits = item.unix_mode().unwrap_or(if item.is_dir() {
            DIRECTORY | 0o755
        } else {
            REGULAR | 0o644
        });
        let size = item.size();
        total = total
            .checked_add(size)
            .filter(|n| *n <= MAX_ZIP)
            .ok_or_else(|| bad("it is too large"))?;
        match mode_bits & TYPE {
            DIRECTORY => {
                folder(root, &name)?;
            }
            LINK => {
                if size > 4096 || item.compressed_size() != size {
                    return Err(bad("a link"));
                }
                let mut destination = String::new();
                (&mut item)
                    .take(size + 1)
                    .read_to_string(&mut destination)?;
                if destination.len() as u64 != size {
                    return Err(bad("a link"));
                }
                let destination = PathBuf::from(destination);
                check_link(&name, &destination)?;
                links.push((name, destination));
            }
            REGULAR | 0 => {
                let mut output = create(&target(root, &name)?, (mode_bits & 0o755) | 0o600)?;
                let got = io::copy(&mut item.take(size + 1), &mut output)?;
                if got != size {
                    return Err(bad("a file isn't the size it says"));
                }
            }
            _ => return Err(bad("an entry isn't a file")),
        }
    }
    for (name, destination) in links {
        link(&destination, &target(root, &name)?)?;
    }
    links_inside(root)
}

fn be32(bytes: &[u8], at: usize) -> io::Result<u32> {
    bytes
        .get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or_else(|| bad("its package header"))
}
fn rpm_header(data: &[u8], at: usize) -> io::Result<(usize, HashMap<u32, &str>)> {
    if data.get(at..at + 4) != Some(&[0x8e, 0xad, 0xe8, 1]) {
        return Err(bad("its package header"));
    }
    let count = be32(data, at + 8)? as usize;
    let size = be32(data, at + 12)? as usize;
    let store = at + 16 + count * 16;
    let end = store
        .checked_add(size)
        .filter(|e| count <= 100_000 && *e <= data.len())
        .ok_or_else(|| bad("its package header"))?;
    let mut strings = HashMap::new();
    for i in 0..count {
        let entry = at + 16 + i * 16;
        let offset = be32(data, entry + 8)? as usize;
        if be32(data, entry + 4)? != 6 || offset >= size {
            continue;
        }
        let text = &data[store + offset..end];
        let zero = text
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| bad("its package header"))?;
        strings.insert(
            be32(data, entry)?,
            std::str::from_utf8(&text[..zero]).map_err(bad)?,
        );
    }
    Ok((end, strings))
}
fn payload(data: &[u8]) -> io::Result<Vec<u8>> {
    if data.len() < 96 || be32(data, 0)? != 0xedabeedb {
        return Err(bad("it isn't a package"));
    }
    let (signature, _) = rpm_header(data, 96)?;
    let (end, header) = rpm_header(data, (signature + 7) & !7)?;
    if header.get(&1124).copied().unwrap_or("cpio") != "cpio" {
        return Err(bad("its package format"));
    }
    let compressed = &data[end..];
    let reader: Box<dyn Read> = match header.get(&1125).copied().unwrap_or("gzip") {
        "xz" => Box::new(xz2::read::XzDecoder::new(compressed)),
        "gzip" => Box::new(flate2::read::GzDecoder::new(compressed)),
        other => return Err(bad(format!("files compressed with {other}"))),
    };
    let mut archive = Vec::new();
    reader.take(MAX_PACKAGE + 1).read_to_end(&mut archive)?;
    if archive.len() as u64 > MAX_PACKAGE {
        return Err(bad("the package is too large"));
    }
    Ok(archive)
}
struct Entry<'a> {
    name: &'a str,
    mode: u32,
    data: &'a [u8],
    hard_link: Option<(u32, u32, u32)>,
}
fn entries(archive: &[u8]) -> io::Result<Vec<Entry<'_>>> {
    let mut at = 0;
    let mut found = Vec::new();
    loop {
        let head = archive
            .get(at..at + 110)
            .ok_or_else(|| bad("its files end early"))?;
        if !matches!(&head[..6], b"070701" | b"070702") {
            return Err(bad("its files"));
        }
        let field = |i: usize| -> io::Result<u32> {
            u32::from_str_radix(
                std::str::from_utf8(&head[6 + i * 8..14 + i * 8]).map_err(bad)?,
                16,
            )
            .map_err(bad)
        };
        let mode = field(1)?;
        let size = field(6)? as usize;
        let name_size = field(11)? as usize;
        let start = at + 110;
        let name = archive
            .get(start..start + name_size)
            .filter(|s| s.last() == Some(&0))
            .ok_or_else(|| bad("its files"))?;
        let name = std::str::from_utf8(&name[..name.len() - 1]).map_err(bad)?;
        let data_start = (start + name_size + 3) & !3;
        let data = archive
            .get(data_start..data_start + size)
            .ok_or_else(|| bad("its files end early"))?;
        if name == "TRAILER!!!" {
            break;
        }
        if matches!(mode & TYPE, REGULAR | LINK) {
            let hard_link = if mode & TYPE == REGULAR && field(4)? > 1 {
                Some((field(7)?, field(8)?, field(0)?))
            } else {
                None
            };
            found.push(Entry {
                name,
                mode,
                data,
                hard_link,
            });
        }
        at = (data_start + size + 3) & !3;
    }
    let contents: HashMap<_, _> = found
        .iter()
        .filter_map(|entry| {
            entry
                .hard_link
                .filter(|_| !entry.data.is_empty())
                .map(|key| (key, entry.data))
        })
        .collect();
    for entry in &mut found {
        if let Some(data) = entry.hard_link.and_then(|k| contents.get(&k)) {
            entry.data = data;
        }
    }
    Ok(found)
}
fn safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\0']) && !matches!(name, "." | "..")
}
fn keep_file(path: &Path, data: &[u8], bits: u32) -> io::Result<()> {
    match create(path, bits) {
        Ok(mut file) => file.write_all(data),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}
pub(super) fn package(file: &Path, bundle: &Path) -> io::Result<()> {
    let data = fs::read(file)?;
    let payload = payload(&data)?;
    'for_entry: for entry in entries(&payload)? {
        let name = entry
            .name
            .strip_prefix("./")
            .or_else(|| entry.name.strip_prefix('/'))
            .unwrap_or(entry.name);
        let regular = entry.mode & TYPE == REGULAR;
        if let Some(name) = name
            .strip_prefix("usr/lib64/")
            .filter(|n| safe_name(n) && n.contains(".so"))
        {
            let path = bundle.join("lib").join(name);
            if regular {
                keep_file(&path, entry.data, 0o755)?;
            } else if let Ok(destination) = std::str::from_utf8(entry.data) {
                if safe_name(destination)
                    && destination
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
                {
                    match link(Path::new(destination), &path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        } else if regular
            && let Some(name) = name
                .strip_prefix("usr/share/fonts/")
                .and_then(|n| n.rsplit('/').next())
                .filter(|n| {
                    safe_name(n)
                        && [".ttf", ".otf", ".ttc"]
                            .iter()
                            .any(|ext| n.to_lowercase().ends_with(ext))
                })
        {
            keep_file(&bundle.join("fonts").join(name), entry.data, 0o644)?;
        } else if regular && let Some(name) = name.strip_prefix("usr/share/licenses/") {
            if name.split('/').count() < 2 || !name.split('/').all(safe_name) {
                continue 'for_entry;
            }
            let path = target(&bundle.join("licenses"), Path::new(name))?;
            keep_file(&path, entry.data, 0o644)?;
        }
    }
    Ok(())
}
