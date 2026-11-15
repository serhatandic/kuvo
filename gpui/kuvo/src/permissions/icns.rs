//! A program's icon, read from its bundle for drawing: the PNG inside its `.icns` (every size from
//! 64 points up is kept there as a PNG). Drawing only: nothing here asks macOS anything.

use std::path::Path;

/// The kinds that hold a PNG, the ones nearest the size it's drawn at first.
const KINDS: [&[u8; 4]; 8] = [b"ic08", b"ic13", b"ic09", b"ic07", b"ic14", b"ic12", b"ic10", b"ic11"];
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The PNG in an `.icns` file's bytes that suits a small tile best; none where it holds none.
pub fn png_in(icns: &[u8]) -> Option<&[u8]> {
    if icns.len() < 8 || &icns[..4] != b"icns" {
        return None;
    }
    let mut found: Vec<(&[u8], &[u8])> = Vec::new();
    let mut at = 8;
    // Each entry: its kind (four letters), its length with this header (big-endian), its bytes.
    while at + 8 <= icns.len() {
        let length = u32::from_be_bytes(icns[at + 4..at + 8].try_into().ok()?) as usize;
        if length < 8 || at + length > icns.len() {
            break;
        }
        let bytes = &icns[at + 8..at + length];
        if bytes.starts_with(PNG) {
            found.push((&icns[at..at + 4], bytes));
        }
        at += length;
    }
    KINDS.iter().find_map(|kind| found.iter().find(|(named, _)| named == &kind.as_slice()).map(|(_, bytes)| *bytes)).or_else(|| found.first().map(|(_, bytes)| *bytes))
}

/// The icon of the app bundle at `bundle`, as a PNG's bytes; none where it has none to read.
pub fn of_bundle(bundle: &Path) -> Option<Vec<u8>> {
    let resources = bundle.join("Contents/Resources");
    let mut icons: Vec<_> = std::fs::read_dir(&resources).ok()?.filter_map(|entry| entry.ok().map(|entry| entry.path())).filter(|path| path.extension().is_some_and(|extension| extension == "icns")).collect();
    icons.sort();
    icons.into_iter().find_map(|icon| std::fs::read(icon).ok().and_then(|bytes| png_in(&bytes).map(<[u8]>::to_vec)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
        [kind.as_slice(), &(bytes.len() as u32 + 8).to_be_bytes(), bytes].concat()
    }

    fn icns(entries: &[Vec<u8>]) -> Vec<u8> {
        let body = entries.concat();
        [b"icns".as_slice(), &(body.len() as u32 + 8).to_be_bytes(), &body].concat()
    }

    #[test]
    fn the_png_nearest_a_tile_s_size_is_taken() {
        let (small, large) = ([PNG, b"small"].concat(), [PNG, b"large"].concat());
        let file = icns(&[entry(b"TOC ", b"contents"), entry(b"ic10", &large), entry(b"is32", b"packed bits"), entry(b"ic08", &small)]);
        assert_eq!(png_in(&file), Some(small.as_slice()));
        // Whatever PNG there is, where it's none of the usual kinds.
        assert_eq!(png_in(&icns(&[entry(b"icp6", &large)])), Some(large.as_slice()));
    }

    #[test]
    fn a_file_that_isnt_an_icon_or_holds_no_png_gives_none() {
        assert_eq!(png_in(b""), None);
        assert_eq!(png_in(b"not an icon at all"), None);
        assert_eq!(png_in(&icns(&[entry(b"is32", b"packed bits")])), None);
        // Cut short part way through an entry: what's whole before it is still read.
        let mut cut = icns(&[entry(b"ic08", &[PNG, b"whole"].concat()), entry(b"ic09", &[PNG, b"cut short"].concat())]);
        cut.truncate(cut.len() - 4);
        assert_eq!(png_in(&cut), Some([PNG, b"whole"].concat().as_slice()));
    }
}
