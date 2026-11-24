//! A timed-out WinSock connect can leave a closed loopback port ambiguous.

use std::{ffi::c_void, mem::size_of, net::Ipv4Addr};

const BUFFER_TOO_SMALL: u32 = 122;
const LISTEN: u32 = 2;
const ROW_WORDS: usize = 5;
const MAX_TABLE: u32 = 16 * 1024 * 1024;

#[link(name = "iphlpapi")]
unsafe extern "system" {
    fn GetTcpTable(table: *mut c_void, size: *mut u32, order: i32) -> u32;
}

pub(super) fn closed(port: u16) -> bool {
    let mut bytes = 0;
    // With a null table the OS reports how much space the IPv4 table needs.
    let status = unsafe { GetTcpTable(std::ptr::null_mut(), &mut bytes, 0) };
    if status != BUFFER_TOO_SMALL {
        return false;
    }
    for _ in 0..3 {
        if bytes == 0 || bytes > MAX_TABLE {
            return false;
        }
        // The header and the five fields of each MIB_TCPROW are DWORDs.
        let mut rows = vec![0u32; (bytes as usize).div_ceil(size_of::<u32>())];
        let status = unsafe { GetTcpTable(rows.as_mut_ptr().cast(), &mut bytes, 0) };
        if status == BUFFER_TOO_SMALL {
            continue;
        }
        let count = rows[0] as usize;
        if status != 0 || count > (rows.len() - 1) / ROW_WORDS {
            return false;
        }
        let loopback = u32::from_ne_bytes(Ipv4Addr::LOCALHOST.octets());
        return !rows[1..].chunks_exact(ROW_WORDS).take(count).any(|row| {
            row[0] == LISTEN
                && (row[1] == 0 || row[1] == loopback)
                && u16::from_be(row[2] as u16) == port
        });
    }
    false
}
