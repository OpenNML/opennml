//! Address of the control channel (`--control`).

use std::net::SocketAddr;

/// Reads `IP:PORT`; only loopback addresses (`127.0.0.1`, `::1`) are allowed so that the
/// control channel is never reachable from the network. Port 0 = chosen by the operating system.
pub fn parse_control_addr(text: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = text.trim().parse().map_err(|_| {
        format!("\"{text}\" is not an address of the form IP:PORT (e.g. 127.0.0.1:0)")
    })?;
    if !addr.ip().is_loopback() {
        return Err(format!(
            "control channel allowed on loopback only (127.0.0.1 or ::1), not {}",
            addr.ip()
        ));
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_addresses_are_accepted() {
        assert_eq!(
            parse_control_addr("127.0.0.1:0").unwrap(),
            "127.0.0.1:0".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(parse_control_addr("[::1]:4711").unwrap().port(), 4711);
        for text in ["0.0.0.0:4711", "192.168.1.5:4711", "[::]:1"] {
            let error = parse_control_addr(text).unwrap_err();
            assert!(error.contains("loopback"), "{text}: {error}");
        }
        for text in ["localhost:4711", "127.0.0.1", "abc", ""] {
            let error = parse_control_addr(text).unwrap_err();
            assert!(error.contains("IP:PORT"), "{text}: {error}");
        }
    }
}
