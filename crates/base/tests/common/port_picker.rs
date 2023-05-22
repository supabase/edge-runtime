use std::net::TcpListener;

pub fn get_available_port() -> u16 {
    (8000..9000).find(|port| port_is_available(*port)).unwrap()
}

pub fn port_is_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}
