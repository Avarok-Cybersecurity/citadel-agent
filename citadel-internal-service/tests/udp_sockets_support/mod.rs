//! This process's UDP sockets, found by walking its file descriptors, and a way to kill one in
//! place: its fd is pointed at a fresh socket bound elsewhere, so whatever owned it keeps a valid
//! fd that no longer receives what was sent to the old address. Two agents in one process lose
//! a direct path this way without either being told.

use std::collections::HashSet;
use std::mem::{size_of, zeroed};
use std::net::{SocketAddr, UdpSocket};
use std::os::fd::AsRawFd;

fn max_fd() -> i32 {
    // SAFETY: getrlimit writes only into the struct it is given.
    unsafe {
        let mut limit: libc::rlimit = zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
            (limit.rlim_cur as i64).min(65_536) as i32
        } else {
            4096
        }
    }
}

/// The local address of `fd` if it is a UDP socket.
fn udp_local_addr(fd: i32) -> Option<SocketAddr> {
    // SAFETY: getsockname/getsockopt write only into the buffers passed with their sizes; an fd
    // that is not a socket (or not open) makes them fail, and it is skipped.
    unsafe {
        let mut storage: libc::sockaddr_storage = zeroed();
        let mut len = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getsockname(fd, &mut storage as *mut _ as *mut libc::sockaddr, &mut len) != 0 {
            return None;
        }
        let mut kind: libc::c_int = 0;
        let mut kind_len = size_of::<libc::c_int>() as libc::socklen_t;
        if libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            &mut kind as *mut _ as *mut libc::c_void,
            &mut kind_len,
        ) != 0
            || kind != libc::SOCK_DGRAM
        {
            return None;
        }
        match storage.ss_family as i32 {
            libc::AF_INET => {
                let v4 = &*(&storage as *const _ as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr));
                Some(SocketAddr::new(ip.into(), u16::from_be(v4.sin_port)))
            }
            libc::AF_INET6 => {
                let v6 = &*(&storage as *const _ as *const libc::sockaddr_in6);
                let ip = std::net::Ipv6Addr::from(v6.sin6_addr.s6_addr);
                Some(SocketAddr::new(ip.into(), u16::from_be(v6.sin6_port)))
            }
            _ => None,
        }
    }
}

/// Every UDP socket open in this process, by local address.
pub fn bound() -> HashSet<SocketAddr> {
    (0..max_fd()).filter_map(udp_local_addr).collect()
}

/// Kills every UDP socket not in `keep`; returns how many.
pub fn kill_all_except(keep: &HashSet<SocketAddr>) -> usize {
    let mut killed = 0;
    for fd in 0..max_fd() {
        let Some(addr) = udp_local_addr(fd) else {
            continue;
        };
        if keep.contains(&addr) {
            continue;
        }
        let replacement =
            UdpSocket::bind(SocketAddr::new(addr.ip(), 0)).expect("replacement socket");
        replacement.set_nonblocking(true).expect("nonblocking");
        // SAFETY: `fd` is an open socket in this process; dup2 atomically points it at the
        // replacement, and `replacement` is closed on drop, leaving `fd` its only reference.
        assert_eq!(unsafe { libc::dup2(replacement.as_raw_fd(), fd) }, fd);
        killed += 1;
    }
    killed
}
