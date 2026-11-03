// connect_tcp's HEAD: route the join to an older server through a translating pipeline.
fn backcompat_connect(
    host: &str,
    port: u16,
    relayed: bool,
    read_timeout: std::time::Duration,
) -> core::ops::ControlFlow<Result<(Connection, String, u16), String>> {
    crate::net::backcompat::connect(host, port, relayed, read_timeout)
}
