//! Forced-command entry point: no paths, hostnames, executables or arbitrary commands.
use anyhow::{ensure, Context, Result};
use celld_ctl_core::{Request, Response, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, SSH_COMMAND};
use std::io::Write;

pub fn validate_original_command(value: Option<&str>) -> Result<()> {
    // Absence is useful to the local root operator. An SSH session always has a value.
    ensure!(
        value.is_none() || value == Some(SSH_COMMAND),
        "SSH command rejected; use the fixed celld-ctl-transport command"
    );
    Ok(())
}
pub fn parse_request(bytes: &[u8]) -> Result<Request> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_REQUEST_BYTES,
        "request exceeds size limit or is empty"
    );
    let request: Request = serde_json::from_slice(bytes).context("invalid transport request")?;
    request.validate().map_err(anyhow::Error::msg)?;
    Ok(request)
}
/// Input is one JSON document followed by EOF. poll prevents an SSH client holding a
/// privileged process/operation lock indefinitely; input is read before acquiring lock.
pub fn read_stdin() -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let start = std::time::Instant::now();
    loop {
        let remaining = std::time::Duration::from_secs(10).saturating_sub(start.elapsed());
        ensure!(!remaining.is_zero(), "transport input timeout");
        let mut fd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd, valid throughout this call.
        let ready = unsafe {
            libc::poll(
                &mut fd,
                1,
                remaining.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        ensure!(ready > 0, "transport input timeout");
        let mut buf = [0u8; 4096];
        // SAFETY: buf is writable for exactly its reported length.
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        ensure!(n >= 0, "read transport input");
        let n = n as usize;
        if n == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buf[..n]);
        ensure!(
            bytes.len() <= MAX_REQUEST_BYTES,
            "request exceeds size limit"
        );
    }
}
pub fn write_response(response: Response) -> Result<()> {
    let mut bytes = serde_json::to_vec(&response)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bytes = serde_json::to_vec(&Response::failure("response exceeds size limit"))?;
    }
    bytes.push(b'\n');
    let mut out = std::io::stdout().lock();
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}
