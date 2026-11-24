//! Read macOS's own list of TCP sockets and answer for one port.
//!
//! The kernel publishes every TCP protocol control block through the
//! `net.inet.tcp.pcblist64` sysctl, which any user may read — the same list
//! `netstat` and `lsof` read. The buffer it hands back opens with a
//! `struct xinpgen`, carries one `struct xtcpcb64` per socket, and closes with
//! a second `struct xinpgen`. Every record leads with its own length, so the
//! walk never needs to know the size of the tail of a record it does not read.
//!
//! The structures below mirror the SDK headers, all three of which sit inside
//! `#pragma pack(4)`:
//!
//! - `struct xinpgen` — `netinet/in_pcb.h`
//! - `struct xinpcb64` — `netinet/in_pcb.h`
//! - `struct xsocket64` — `sys/socketvar.h`
//! - `struct xtcpcb64` — `netinet/tcp_var.h`
//!
//! Only the head of `xtcpcb64` is transcribed. Reading `t_state` instead of
//! `so_options` would cost the whole tail, `TCPT_NTIMERS_EXT` included, to
//! learn the same fact.
//!
//! # A wrong transcription is loud, not silent
//!
//! `xsocket64` carries `xso_len`, and the kernel fills it with the real size of
//! that structure. Every record is checked against `size_of::<Xsocket64>()`,
//! and a mismatch answers [`PortStatus::Unknown`] rather than reading fields
//! out of a layout this file no longer describes. The check reaches further
//! than `xsocket64` alone: `xso_len` is read at the offset this file computes
//! for `xi_socket` inside `xinpcb64`, so a field added, removed or resized
//! anywhere ahead of it — `inp_lport` included — moves the read and the value
//! stops being 108. A macOS that moves a field therefore makes `wl` say it
//! cannot tell, instead of answering wrongly. That check is what makes this
//! module safe to write at all.
//!
//! `xinpcb64` carries an `xi_len` of its own, and this sysctl **never fills
//! it**: `inpcb_to_xinpcb64` leaves the field at the zero the record was
//! cleared to, which every record on this machine confirms. So the check on it
//! is "zero, or the size of the mirror here" — today's kernel passes by
//! writing nothing, and a kernel that starts filling the field must fill it
//! with a size this file agrees with.
//!
//! Everything else fails closed the same way: a `sysctl` that fails, a record
//! length that runs past the end of the buffer, a record shorter than the
//! structure, and a walk that never reaches the closing header all answer
//! [`PortStatus::Unknown`]. The walk always runs to the closing header, even
//! after it has found the port, so that a buffer which turns out to be
//! malformed answers [`PortStatus::Unknown`] rather than an answer read out of
//! the part that happened to parse; and the closing header itself has to be
//! there in full, or a buffer cut one byte short would read as a list that
//! ended. [`PortStatus::Free`] is only ever the answer of a walk that
//! completed cleanly and found no listening socket on the port.
//!
//! The parsing half compiles on every platform so that its synthetic-buffer
//! tests run on every platform. Only the `sysctl` call that feeds it is
//! macOS-only.

#![cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "the byte-slice parser compiles everywhere so its synthetic-buffer tests \
                  run everywhere; only the sysctl call that feeds it is macOS-only"
    )
)]

use super::PortStatus;
use std::mem::offset_of;

/// `SO_ACCEPTCONN` (`sys/socket.h`): the socket has had `listen()` called on
/// it. `so_options` is a `short`, so the mask is one too.
const SO_ACCEPTCONN: i16 = 0x0002;

/// Mirror of `struct xinpgen` (`netinet/in_pcb.h`), the header that opens the
/// list and the sentinel that closes it.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Xinpgen {
    xig_len: u32,
    xig_count: u32,
    xig_gen: u64,
    xig_sogen: u64,
}

/// Mirror of `struct xsockbuf` (`sys/socketvar.h`).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Xsockbuf {
    sb_cc: u32,
    sb_hiwat: u32,
    sb_mbcnt: u32,
    sb_mbmax: u32,
    sb_lowat: i32,
    sb_flags: i16,
    sb_timeo: i16,
}

/// Mirror of `struct xsocket64` (`sys/socketvar.h`), the externalized form of
/// a socket. `so_options` is the field this module reads.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Xsocket64 {
    xso_len: u32,
    xso_so: u64,
    so_type: i16,
    so_options: i16,
    so_linger: i16,
    so_state: i16,
    so_pcb: u64,
    xso_protocol: i32,
    xso_family: i32,
    so_qlen: i16,
    so_incqlen: i16,
    so_qlimit: i16,
    so_timeo: i16,
    so_error: u16,
    so_pgid: i32,
    so_oobmark: u32,
    so_rcv: Xsockbuf,
    so_snd: Xsockbuf,
    so_uid: u32,
}

/// Mirror of `struct inpcb64_list_entry` (`netinet/in_pcb.h`).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Inpcb64ListEntry {
    le_next: u64,
    le_prev: u64,
}

/// Mirror of the `inp_dependfaddr` / `inp_dependladdr` unions
/// (`netinet/in_pcb.h`).
///
/// Both arms — `struct in_addr_4in6` and `struct in6_addr` — are sixteen bytes
/// wide and four-byte aligned, so four `u32` reproduce the union exactly.
/// Sixteen `u8` would not: the alignment would drop to one and move every
/// field after it.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct InpcbDependAddr {
    words: [u32; 4],
}

/// Mirror of the anonymous `inp_depend4` structure (`netinet/in_pcb.h`).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct InpDepend4 {
    inp4_ip_tos: u8,
}

/// Mirror of the anonymous `inp_depend6` structure (`netinet/in_pcb.h`).
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct InpDepend6 {
    inp6_hlim: u8,
    inp6_cksum: i32,
    inp6_ifindex: u16,
    inp6_hops: i16,
}

/// Mirror of `struct xinpcb64` (`netinet/in_pcb.h`), the externalized form of
/// an internet protocol control block.
///
/// `inp_lport` is the local port **in network byte order**.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Xinpcb64 {
    xi_len: u64,
    xi_inpp: u64,
    inp_fport: u16,
    inp_lport: u16,
    inp_list: Inpcb64ListEntry,
    inp_ppcb: u64,
    inp_pcbinfo: u64,
    inp_portlist: Inpcb64ListEntry,
    inp_phd: u64,
    inp_gencnt: u64,
    inp_flags: i32,
    inp_flow: u32,
    inp_vflag: u8,
    inp_ip_ttl: u8,
    inp_ip_p: u8,
    inp_dependfaddr: InpcbDependAddr,
    inp_dependladdr: InpcbDependAddr,
    inp_depend4: InpDepend4,
    inp_depend6: InpDepend6,
    xi_socket: Xsocket64,
    xi_alignment_hack: u64,
}

/// The head of `struct xtcpcb64` (`netinet/tcp_var.h`) — everything this
/// module reads.
///
/// The tail carries the TCP state machine and is skipped over by `xt_len`, so
/// nothing here depends on `TCPT_NTIMERS_EXT` or on the size of `tcp_seq`.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct Xtcpcb64Head {
    xt_len: u32,
    xt_inpcb: Xinpcb64,
}

// The sizes and offsets the macOS SDK reports on a 64-bit host. A transcription
// that drifts from the headers fails to compile here rather than reading a
// field out of the wrong place at run time.
const _: () = assert!(size_of::<Xinpgen>() == 24);
const _: () = assert!(size_of::<Xsockbuf>() == 24);
const _: () = assert!(size_of::<Xsocket64>() == 108);
const _: () = assert!(size_of::<Xinpcb64>() == 260);
const _: () = assert!(size_of::<Xtcpcb64Head>() == 264);
const _: () = assert!(offset_of!(Xsocket64, so_options) == 14);
const _: () = assert!(offset_of!(Xinpcb64, inp_lport) == 18);
const _: () = assert!(offset_of!(Xinpcb64, xi_socket) == 144);
const _: () = assert!(offset_of!(Xtcpcb64Head, xt_inpcb) == 4);

/// Answer for `port` from the kernel's list of TCP sockets.
///
/// A `sysctl` this call cannot complete answers [`PortStatus::Unknown`]: the
/// question was never reached, so nothing is known about the port.
#[cfg(target_os = "macos")]
pub fn port_status(port: u16) -> PortStatus {
    match read_pcblist() {
        Some(list) => parse_port_status(&list, port),
        None => PortStatus::Unknown,
    }
}

/// The sysctl that publishes the 64-bit form of the TCP protocol control
/// blocks. NUL-terminated for `sysctlbyname`.
#[cfg(target_os = "macos")]
const PCBLIST64: &[u8] = b"net.inet.tcp.pcblist64\0";

/// Extra bytes to ask for beyond the size the kernel just quoted, on top of a
/// one-eighth proportional margin. The list can gain sockets between the call
/// that sizes it and the call that reads it.
#[cfg(target_os = "macos")]
const SIZING_SLACK_BYTES: usize = 4096;

/// How many times to re-ask when the list outgrows the buffer anyway.
#[cfg(target_os = "macos")]
const MAX_SIZING_ATTEMPTS: u32 = 4;

/// Take one snapshot of `net.inet.tcp.pcblist64`.
///
/// Sizing a sysctl takes two calls and the answer can grow between them, so an
/// `ENOMEM` is retried a bounded number of times. Every other failure, and an
/// exhausted retry budget, answers `None` — which the caller reads as
/// [`PortStatus::Unknown`].
#[cfg(target_os = "macos")]
fn read_pcblist() -> Option<Vec<u8>> {
    for _ in 0..MAX_SIZING_ATTEMPTS {
        let mut needed: libc::size_t = 0;

        // SAFETY: `sysctlbyname` reads a NUL-terminated name, which
        // `PCBLIST64` is. With a null output buffer it writes nothing but the
        // required byte count through `oldlenp`, which points at a live local.
        // The two `newp`/`newlen` arguments are null and zero, so the call
        // writes no kernel state.
        let sized = unsafe {
            libc::sysctlbyname(
                PCBLIST64.as_ptr().cast(),
                std::ptr::null_mut(),
                &raw mut needed,
                std::ptr::null_mut(),
                0,
            )
        };
        if sized != 0 || needed == 0 {
            return None;
        }

        let capacity = needed
            .checked_add(needed / 8)?
            .checked_add(SIZING_SLACK_BYTES)?;
        let mut list = vec![0_u8; capacity];
        let mut written: libc::size_t = capacity;

        // SAFETY: as above, plus `list` owns exactly `capacity` writable bytes
        // and `written` tells the kernel so; it writes no more than that and
        // reports what it wrote back through the same pointer.
        let read = unsafe {
            libc::sysctlbyname(
                PCBLIST64.as_ptr().cast(),
                list.as_mut_ptr().cast(),
                &raw mut written,
                std::ptr::null_mut(),
                0,
            )
        };
        if read == 0 {
            if written > capacity {
                return None;
            }
            list.truncate(written);
            return Some(list);
        }

        if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOMEM) {
            return None;
        }
    }

    None
}

/// Read the host-order `u_int32_t` that opens a record.
///
/// The lengths in this list are host order; only `inp_lport` is not.
fn read_record_length(list: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(size_of::<u32>())?;
    let bytes: [u8; 4] = list.get(offset..end)?.try_into().ok()?;
    Some(u32::from_ne_bytes(bytes))
}

/// Say whether one record is a socket listening on `port`.
///
/// `None` means the record did not match this file's transcription of the
/// kernel's layout — the one answer that must never be read as "free".
fn record_listens_on(record: &[u8], port: u16) -> Option<bool> {
    if record.len() < size_of::<Xtcpcb64Head>() {
        return None;
    }

    // SAFETY: `Xtcpcb64Head` is a `repr(C, packed(4))` mirror of plain kernel
    // data. Every field is an integer, so every bit pattern is a valid value
    // and the type owns nothing that could be dropped twice. The length check
    // above proves `record` holds at least `size_of::<Xtcpcb64Head>()`
    // readable bytes, and `read_unaligned` places no alignment requirement on
    // the source.
    let head: Xtcpcb64Head = unsafe { std::ptr::read_unaligned(record.as_ptr().cast()) };

    // Every field is copied out by value. A reference to a field of a packed
    // structure is undefined behaviour, and a hard error in modern Rust.
    let xi_len = head.xt_inpcb.xi_len;
    let xso_len = head.xt_inpcb.xi_socket.xso_len;

    // The kernel's own account of each structure's size, against this file's.
    // `xso_len` is filled on every record; `xi_len` is not filled by this
    // sysctl at all, so a zero there is the kernel saying nothing rather than
    // the kernel disagreeing.
    let inpcb_agrees =
        xi_len == 0 || usize::try_from(xi_len).is_ok_and(|len| len == size_of::<Xinpcb64>());
    let socket_agrees = usize::try_from(xso_len).is_ok_and(|len| len == size_of::<Xsocket64>());
    if !inpcb_agrees || !socket_agrees {
        return None;
    }

    let so_options = head.xt_inpcb.xi_socket.so_options;
    let local_port = head.xt_inpcb.inp_lport;

    Some(so_options & SO_ACCEPTCONN != 0 && u16::from_be(local_port) == port)
}

/// Answer for `port` from one snapshot of the kernel's TCP socket list.
///
/// Split from the `sysctl` call so a synthetic buffer can exercise it on any
/// platform, kernel or no kernel.
///
/// The walk runs to the closing header even once it has found the port, so
/// that a list which stops making sense further along answers
/// [`PortStatus::Unknown`]. Half a list read cleanly is not a reading of the
/// list.
fn parse_port_status(list: &[u8], port: u16) -> PortStatus {
    let header_len = size_of::<Xinpgen>();

    // The opening `xinpgen` says where the first record starts.
    let Some(opening_len) = read_record_length(list, 0) else {
        return PortStatus::Unknown;
    };
    let mut offset = opening_len as usize;
    if offset < header_len {
        return PortStatus::Unknown;
    }

    let mut found_listener = false;

    loop {
        let Some(record_len) = read_record_length(list, offset) else {
            // The walk ran off the end of the buffer without reaching the
            // closing header.
            return PortStatus::Unknown;
        };
        let record_len = record_len as usize;

        if record_len <= header_len {
            // The closing `xinpgen`. Only a whole one ends the walk: a buffer
            // cut short mid-header, or a sentinel that is not the size of the
            // header, is a list this run never reached the end of.
            let sentinel_is_whole = record_len == header_len
                && offset
                    .checked_add(header_len)
                    .is_some_and(|end| end <= list.len());
            if !sentinel_is_whole {
                return PortStatus::Unknown;
            }
            return if found_listener {
                PortStatus::InUse
            } else {
                PortStatus::Free
            };
        }
        if record_len < size_of::<Xtcpcb64Head>() {
            return PortStatus::Unknown;
        }

        let Some(next) = offset.checked_add(record_len) else {
            return PortStatus::Unknown;
        };
        if next > list.len() {
            return PortStatus::Unknown;
        }

        match record_listens_on(&list[offset..next], port) {
            Some(listening) => found_listener |= listening,
            None => return PortStatus::Unknown,
        }

        offset = next;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_port_status, PortStatus, Xinpcb64, Xinpgen, Xsocket64, Xtcpcb64Head, SO_ACCEPTCONN,
    };

    /// Every record these tests build declares exactly the head this module
    /// reads, which is the shortest record the walk accepts.
    const RECORD_LEN: usize = size_of::<Xtcpcb64Head>();

    /// A structure whose every field is an integer, zeroed.
    ///
    /// The mirrors carry padding, so the bytes are taken from a value the
    /// whole width of which was written rather than field by field.
    fn zeroed<T: Copy>() -> T {
        // SAFETY: `T` is only ever instantiated here with one of this module's
        // mirrors. Each is a `repr(C, packed(4))` structure of integers and
        // arrays of integers, for which the all-zero bit pattern is a valid
        // value, and none of them owns a resource.
        unsafe { std::mem::zeroed() }
    }

    /// The bytes of one value, as the kernel would have written them.
    fn bytes_of<T: Copy>(value: &T) -> Vec<u8> {
        // SAFETY: `value` points at one initialized `T` of `size_of::<T>()`
        // bytes, produced by `zeroed` above, so the whole span — padding
        // included — is initialized memory. Reading it as `u8` imposes no
        // alignment requirement and the borrow of `value` keeps it alive.
        let raw = unsafe {
            std::slice::from_raw_parts(std::ptr::from_ref(value).cast::<u8>(), size_of::<T>())
        };
        raw.to_vec()
    }

    /// The header that opens the list and the sentinel that closes it.
    fn header_bytes() -> Vec<u8> {
        let mut header: Xinpgen = zeroed();
        header.xig_len = u32::try_from(size_of::<Xinpgen>()).expect("xinpgen fits in a u32");
        bytes_of(&header)
    }

    /// One record for a socket on `port`, listening or not.
    fn record(port: u16, listening: bool) -> Xtcpcb64Head {
        let mut head: Xtcpcb64Head = zeroed();
        head.xt_len = u32::try_from(RECORD_LEN).expect("a record head fits in a u32");
        head.xt_inpcb.xi_len =
            u64::try_from(size_of::<Xinpcb64>()).expect("xinpcb64 fits in a u64");
        head.xt_inpcb.xi_socket.xso_len =
            u32::try_from(size_of::<Xsocket64>()).expect("xsocket64 fits in a u32");
        head.xt_inpcb.inp_lport = port.to_be();
        head.xt_inpcb.xi_socket.so_options = if listening { SO_ACCEPTCONN } else { 0 };
        head
    }

    /// A whole list: opening header, the records, closing header.
    fn list_of(records: &[Xtcpcb64Head]) -> Vec<u8> {
        let mut list = header_bytes();
        for record in records {
            list.extend_from_slice(&bytes_of(record));
        }
        list.extend_from_slice(&header_bytes());
        list
    }

    #[test]
    fn a_listening_socket_on_the_port_is_in_use() {
        let list = list_of(&[record(8080, true)]);

        assert_eq!(parse_port_status(&list, 8080), PortStatus::InUse);
    }

    #[test]
    fn a_listening_socket_on_another_port_leaves_the_port_free() {
        let list = list_of(&[record(8080, true)]);

        assert_eq!(parse_port_status(&list, 9090), PortStatus::Free);
    }

    #[test]
    fn a_socket_that_never_listened_leaves_the_port_free() {
        // A connected socket holds a local port without accepting on it, which
        // is not what `wl` was asked about.
        let list = list_of(&[record(8080, false)]);

        assert_eq!(parse_port_status(&list, 8080), PortStatus::Free);
    }

    #[test]
    fn a_match_is_found_after_records_that_do_not_match() {
        let list = list_of(&[record(80, false), record(443, true), record(8080, true)]);

        assert_eq!(parse_port_status(&list, 8080), PortStatus::InUse);
    }

    #[test]
    fn an_empty_list_leaves_the_port_free() {
        assert_eq!(parse_port_status(&list_of(&[]), 8080), PortStatus::Free);
    }

    #[test]
    fn a_disagreeing_xi_len_is_unknown() {
        // The kernel's account of `xinpcb64` no longer matches this file's, so
        // every offset past it is a guess.
        let mut head = record(8080, true);
        head.xt_inpcb.xi_len = u64::try_from(size_of::<Xinpcb64>() + 8).expect("fits in a u64");

        assert_eq!(
            parse_port_status(&list_of(&[head]), 8080),
            PortStatus::Unknown
        );
    }

    #[test]
    fn a_disagreeing_xso_len_is_unknown() {
        let mut head = record(8080, true);
        head.xt_inpcb.xi_socket.xso_len =
            u32::try_from(size_of::<Xsocket64>() + 4).expect("fits in a u32");

        assert_eq!(
            parse_port_status(&list_of(&[head]), 8080),
            PortStatus::Unknown
        );
    }

    #[test]
    fn a_record_length_that_overruns_the_buffer_is_unknown() {
        let mut list = list_of(&[record(8080, false)]);
        let record_start = size_of::<Xinpgen>();
        let overrun = u32::try_from(list.len() + 8).expect("fits in a u32");
        list[record_start..record_start + size_of::<u32>()].copy_from_slice(&overrun.to_ne_bytes());

        assert_eq!(parse_port_status(&list, 8080), PortStatus::Unknown);
    }

    #[test]
    fn a_record_shorter_than_the_head_is_unknown() {
        let mut list = list_of(&[record(8080, false)]);
        let record_start = size_of::<Xinpgen>();
        // Longer than the closing header, so the walk reads it as a record,
        // and shorter than the head this module reads.
        let too_short = u32::try_from(size_of::<Xtcpcb64Head>() - 4).expect("fits in a u32");
        list[record_start..record_start + size_of::<u32>()]
            .copy_from_slice(&too_short.to_ne_bytes());

        assert_eq!(parse_port_status(&list, 8080), PortStatus::Unknown);
    }

    #[test]
    fn a_truncated_buffer_is_unknown() {
        let list = list_of(&[record(8080, true)]);

        // Every cut leaves the walk short of the end of the list: before the
        // opening header, at the first record, inside that record, and one
        // byte inside the closing header.
        let cuts = [
            0,
            1,
            size_of::<Xinpgen>(),
            list.len() - size_of::<Xinpgen>() - 4,
            list.len() - 1,
        ];
        for cut in cuts {
            let truncated = &list[..cut];
            assert_eq!(
                parse_port_status(truncated, 8080),
                PortStatus::Unknown,
                "a buffer of {cut} bytes carries no complete list, so it cannot answer"
            );
        }
    }

    #[test]
    fn an_opening_header_shorter_than_itself_is_unknown() {
        let mut list = list_of(&[record(8080, false)]);
        list[..size_of::<u32>()].copy_from_slice(&8_u32.to_ne_bytes());

        assert_eq!(parse_port_status(&list, 8080), PortStatus::Unknown);
    }

    /// Pin this file's transcription against the running kernel.
    ///
    /// The synthetic tests above build their records from the same mirrors the
    /// parser reads, so they agree with a wrong transcription as readily as
    /// with a right one. This one does not: a moved field, or a local port
    /// read without its byte-order conversion, fails it.
    #[cfg(target_os = "macos")]
    #[test]
    fn reads_a_live_listening_socket_from_the_kernel() {
        // The kernel picks the port, so two copies of this test that run at
        // the same time never ask about the same one.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("should be able to bind an ephemeral port");
        let port = listener
            .local_addr()
            .expect("bound listener must have a local address")
            .port();

        assert_eq!(
            super::port_status(port),
            PortStatus::InUse,
            "this test is listening on port {port}, so the kernel's socket table \
             holds a listening socket for it"
        );

        drop(listener);

        assert_eq!(
            super::port_status(port),
            PortStatus::Free,
            "this test released port {port}, so the kernel's socket table holds \
             no listening socket for it"
        );
    }
}
