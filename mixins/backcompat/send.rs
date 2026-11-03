// ConnWriter::send's HEAD: an older server's version of the packet, or the packet as it is.
fn backcompat_send(writer: &mut ConnWriter, body: &[u8]) -> core::ops::ControlFlow<Result<()>> {
    crate::connection::backcompat::on_send(writer, body)
}
