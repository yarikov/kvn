use anyhow::{Context, Result};
use std::net::TcpListener;

pub fn allocate_loopback_port() -> Result<u16> {
    let listener =
        TcpListener::bind("127.0.0.1:0").context("Failed to allocate a free loopback port")?;
    let port = listener
        .local_addr()
        .context("Failed to read the allocated loopback port")?
        .port();
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REBIND_ATTEMPTS: usize = 5;

    #[test]
    fn allocated_port_is_released_for_rebinding() {
        for attempt in 1..=REBIND_ATTEMPTS {
            let port = allocate_loopback_port().unwrap();
            assert_ne!(port, 0);
            match TcpListener::bind(("127.0.0.1", port)) {
                Ok(_) => return,
                Err(error)
                    if error.kind() == std::io::ErrorKind::AddrInUse
                        && attempt < REBIND_ATTEMPTS => {}
                Err(error) => panic!("allocated port {port} could not be rebound: {error}"),
            }
        }
        panic!("every allocated port stayed bound; the listener is not being released");
    }

    #[test]
    fn allocated_port_differs_from_a_bound_port() {
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let held_port = held.local_addr().unwrap().port();
        assert_ne!(allocate_loopback_port().unwrap(), held_port);
    }
}
