use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};

pub(super) fn addresses(name: &str) -> io::Result<Vec<SocketAddr>> {
    let mut interfaces = if_addrs::get_if_addrs()?
        .into_iter()
        .filter(|interface| interface.name == name)
        .collect::<Vec<_>>();
    interfaces.sort_by_key(|interface| interface.is_loopback() || interface.is_link_local());

    let mut addresses = Vec::with_capacity(interfaces.len());
    for interface in interfaces {
        let address = match interface.ip() {
            IpAddr::V4(address) => SocketAddr::V4(SocketAddrV4::new(address, 0)),
            IpAddr::V6(address) => SocketAddr::V6(SocketAddrV6::new(
                address,
                0,
                0,
                interface.index.unwrap_or(0),
            )),
        };
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }

    if addresses.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("BindInterface {name} does not exist or has no IP address"),
        ))
    } else {
        Ok(addresses)
    }
}

#[cfg(feature = "libssh")]
pub(super) fn host(address: &SocketAddr) -> String {
    match address {
        SocketAddr::V4(address) => address.ip().to_string(),
        SocketAddr::V6(address) if address.scope_id() == 0 => address.ip().to_string(),
        SocketAddr::V6(address) => format!("{}%{}", address.ip(), address.scope_id()),
    }
}
