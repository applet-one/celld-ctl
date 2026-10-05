//! Owner SSH transport entry point: no paths, hostnames or arbitrary commands in input.
use anyhow::{ensure, Context, Result};
use celld_ctl_core::{
    PreparedBundle, Request, Response, MAX_BUNDLE_BYTES, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
};
use std::io::Write;

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
struct InputBudget {
    limit: usize,
    timeout: std::time::Duration,
    saw_newline: bool,
}
impl InputBudget {
    fn new() -> Self {
        Self {
            limit: MAX_REQUEST_BYTES,
            timeout: std::time::Duration::from_secs(30),
            saw_newline: false,
        }
    }
    fn observe(&mut self, bytes: &[u8]) -> Result<()> {
        if !self.saw_newline {
            if let Some(newline) = bytes.iter().position(|b| *b == b'\n') {
                self.saw_newline = true;
                if newline <= MAX_REQUEST_BYTES {
                    if let Ok(Request::Deploy { bundle_size, .. }) =
                        parse_request(&bytes[..newline])
                    {
                        ensure!(bundle_size <= MAX_BUNDLE_BYTES, "bundle exceeds size limit");
                        self.limit = newline + 1 + bundle_size;
                        // Extend only a validated deploy header, measured from the
                        // original request start; this is not a sliding idle timeout.
                        self.timeout = std::time::Duration::from_secs(180);
                    }
                }
            }
        }
        ensure!(
            bytes.len() <= self.limit,
            "request or deploy body exceeds its size limit"
        );
        Ok(())
    }
}
/// Read and validate the bounded upload before acquiring the host mutation lock.
/// poll+raw read avoids stdio buffering ambiguity and clients holding root forever.
pub fn read_stdin() -> Result<Incoming> {
    let mut bytes = Vec::new();
    let start = std::time::Instant::now();
    let mut budget = InputBudget::new();
    loop {
        let remaining = budget.timeout.saturating_sub(start.elapsed());
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
        budget.observe(&bytes)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_validated_deploy_headers_extend_the_fixed_input_deadline() {
        let mut budget = InputBudget::new();
        budget.observe(br#"{"op":"deploy""#).unwrap();
        assert_eq!(budget.timeout.as_secs(), 30);
        let mut header=serde_json::to_vec(&json!({"op":"deploy","slug":"app","celld_version":"0.6.1","version_id":"abc","bundle_size":100})).unwrap();
        header.push(b'\n');
        budget.observe(&header).unwrap();
        assert_eq!(budget.timeout.as_secs(), 180);
        assert_eq!(budget.limit, header.len() + 100);
        assert_eq!(
            budget
                .timeout
                .saturating_sub(std::time::Duration::from_secs(54))
                .as_secs(),
            126
        );
        header.extend([b' '; 101]);
        assert!(budget.observe(&header).is_err());
        for raw in [br#"{"op":"status","slug":"app"}"#.as_slice(),br#"{"op":"deploy","slug":"app","celld_version":"latest","version_id":"abc","bundle_size":100}"#] {
            let mut budget=InputBudget::new();let mut bytes=raw.to_vec();bytes.push(b'\n');budget.observe(&bytes).unwrap();
            assert_eq!(budget.timeout.as_secs(),30);assert_eq!(budget.limit,MAX_REQUEST_BYTES);
            assert!(budget.timeout.saturating_sub(std::time::Duration::from_secs(31)).is_zero());
        }
    }
}
