use std::ffi::c_char;
use std::fs;
use std::io;
use std::mem;

use anyhow::{bail, Result};

const ETHTOOL_GWOL: u32 = 0x0000_0006;
const ETHTOOL_SWOL: u32 = 0x0000_0008;
const WAKE_MAGIC: u32 = 0x0000_0020;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct EthtoolWolinfo {
    cmd: u32,
    supported: u32,
    wolopts: u32,
    sopass: [u8; 6],
}

pub fn enabled() -> bool {
    for interface in interfaces() {
        if let Ok(info) = get_wol(&interface) {
            if info.supported & WAKE_MAGIC != 0 && info.wolopts & WAKE_MAGIC != 0 {
                return true;
            }
        }
    }
    false
}

pub fn capable() -> bool {
    for interface in interfaces() {
        if let Ok(info) = get_wol(&interface) {
            if info.supported & WAKE_MAGIC != 0 {
                return true;
            }
        }
    }
    false
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("changing womp requires root (try sudo)");
    }

    let mut capable_interfaces = 0;
    let mut errors = Vec::new();

    for interface in interfaces() {
        let info = match get_wol(&interface) {
            Ok(info) => info,
            Err(_) => continue,
        };
        if info.supported & WAKE_MAGIC == 0 {
            continue;
        }
        capable_interfaces += 1;

        let wolopts = if enabled {
            info.wolopts | WAKE_MAGIC
        } else {
            info.wolopts & !WAKE_MAGIC
        };
        if wolopts == info.wolopts {
            continue;
        }

        if let Err(err) = set_wol(&interface, wolopts, info.sopass) {
            errors.push(format!("{interface}: {err}"));
        }
    }

    if capable_interfaces == 0 {
        if enabled {
            bail!("no Wake-on-LAN capable network interface found");
        }
        return Ok(());
    }
    if !errors.is_empty() {
        bail!("failed to set Wake-on-LAN: {}", errors.join("; "));
    }
    Ok(())
}

fn interfaces() -> Vec<String> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name != "lo").then_some(name)
        })
        .collect();
    names.sort();
    names
}

fn get_wol(interface: &str) -> io::Result<EthtoolWolinfo> {
    ethtool_wol(interface, None)
}

fn set_wol(interface: &str, wolopts: u32, sopass: [u8; 6]) -> io::Result<()> {
    ethtool_wol(
        interface,
        Some(EthtoolWolinfo {
            cmd: ETHTOOL_SWOL,
            supported: 0,
            wolopts,
            sopass,
        }),
    )?;
    Ok(())
}

fn ethtool_wol(interface: &str, request: Option<EthtoolWolinfo>) -> io::Result<EthtoolWolinfo> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut ifr: libc::ifreq = unsafe { mem::zeroed() };
    let name = interface.as_bytes();
    if name.len() >= libc::IFNAMSIZ {
        unsafe { libc::close(fd) };
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "network interface name is too long",
        ));
    }
    for (index, byte) in name.iter().enumerate() {
        ifr.ifr_name[index] = *byte as c_char;
    }

    let mut info = request.unwrap_or(EthtoolWolinfo {
        cmd: ETHTOOL_GWOL,
        ..Default::default()
    });
    ifr.ifr_ifru.ifru_data = &mut info as *mut EthtoolWolinfo as *mut c_char;

    let result = unsafe { libc::ioctl(fd, libc::SIOCETHTOOL, &mut ifr) };
    let error = io::Error::last_os_error();
    unsafe { libc::close(fd) };

    if result < 0 {
        Err(error)
    } else {
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wolinfo_matches_kernel_layout() {
        assert_eq!(mem::size_of::<EthtoolWolinfo>(), 20);
    }
}
