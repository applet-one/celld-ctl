//! Forced-command entry point: no paths, hostnames, executables or arbitrary commands.
use anyhow::{ensure, Context, Result};
use celld_ctl_core::{
    PreparedBundle, Request, Response, MAX_BUNDLE_BYTES, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
    SSH_COMMAND,
};
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
#[derive(Debug)]
pub struct Incoming {
    pub request: Request,
    pub bundle: Option<PreparedBundle>,
}
/// A deploy uses one single-line JSON header, newline, exact-length JSON body,
/// then EOF. Other operations retain the one-JSON-document-plus-EOF protocol.
pub fn parse_input(bytes: &[u8]) -> Result<Incoming> {
    if let Some(newline) = bytes.iter().position(|b| *b == b'\n') {
        if newline <= MAX_REQUEST_BYTES {
            if let Ok(request @ Request::Deploy { .. }) = parse_request(&bytes[..newline]) {
                let Request::Deploy { bundle_size, .. } = &request else {
                    unreachable!()
                };
                ensure!(
                    bytes.len() - newline - 1 == *bundle_size,
                    "deploy body length does not match bundle_size"
                );
                let bundle: PreparedBundle = serde_json::from_slice(&bytes[newline + 1..])
                    .context("invalid prepared bundle JSON")?;
                let bundle = bundle.normalize().map_err(anyhow::Error::msg)?;
                return Ok(Incoming {
                    request,
                    bundle: Some(bundle),
                });
            }
        }
    }
    let request = parse_request(bytes)?;
    ensure!(
        !matches!(request, Request::Deploy { .. }),
        "deploy needs a newline-delimited metadata header and exact-length body"
    );
    Ok(Incoming {
        request,
        bundle: None,
    })
}
/// Read and validate the bounded upload before acquiring the host mutation lock.
/// poll+raw read avoids stdio buffering ambiguity and clients holding root forever.
pub fn read_stdin() -> Result<Incoming> {
    let mut bytes = Vec::new();
    let start = std::time::Instant::now();
    let mut limit = MAX_REQUEST_BYTES;
    let mut saw_newline = false;
    loop {
        let remaining = std::time::Duration::from_secs(30).saturating_sub(start.elapsed());
        ensure!(!remaining.is_zero(), "transport input timeout");
        let mut fd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut fd,
                1,
                remaining.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        ensure!(ready > 0, "transport input timeout");
        let mut buf = [0u8; 8192];
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        ensure!(n >= 0, "read transport input");
        let n = n as usize;
        if n == 0 {
            return parse_input(&bytes);
        }
        bytes.extend_from_slice(&buf[..n]);
        if !saw_newline {
            if let Some(newline) = bytes.iter().position(|b| *b == b'\n') {
                saw_newline = true;
                if newline <= MAX_REQUEST_BYTES {
                    if let Ok(Request::Deploy { bundle_size, .. }) =
                        parse_request(&bytes[..newline])
                    {
                        ensure!(bundle_size <= MAX_BUNDLE_BYTES, "bundle exceeds size limit");
                        limit = newline + 1 + bundle_size;
                    }
                }
            }
        }
        ensure!(
            bytes.len() <= limit,
            "request or deploy body exceeds its size limit"
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
