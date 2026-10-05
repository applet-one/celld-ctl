//! Small path-style SigV4 signer shared by operator bootstrap and pointer reads.
//! Request destinations must be preselected by the host, never by SSH input.
use crate::config::storage_origin;
use anyhow::{ensure, Context, Result};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts arbitrary key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

fn auth_headers(
    method: &reqwest::Method,
    host: &str,
    path: &str,
    region: &str,
    credentials: &BTreeMap<String, String>,
    body: &[u8],
    stamp: &str,
) -> Result<BTreeMap<String, String>> {
    ensure!(
        path.starts_with('/')
            && path.len() <= 1024
            && path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-._~".contains(&b)),
        "invalid signed S3 path"
    );
    ensure!(
        !region.is_empty()
            && region.len() <= 64
            && region
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "invalid S3 region"
    );
    ensure!(
        stamp.len() == 16 && stamp.ends_with('Z'),
        "invalid signing timestamp"
    );
    let access = credentials
        .get("AWS_ACCESS_KEY_ID")
        .context("missing S3 access key")?;
    let secret = credentials
        .get("AWS_SECRET_ACCESS_KEY")
        .context("missing S3 secret key")?;
    ensure!(
        !access.is_empty()
            && !secret.is_empty()
            && access.len() <= 256
            && secret.len() <= 256
            && [access, secret].iter().all(|s| s
                .bytes()
                .all(|b| b.is_ascii_graphic() && !b"\r\n\t \\\"'".contains(&b))),
        "invalid S3 credential encoding"
    );
    let hash = hex::encode(Sha256::digest(body));
    let mut headers = BTreeMap::from([
        ("host".to_owned(), host.to_owned()),
        ("x-amz-content-sha256".to_owned(), hash.clone()),
        ("x-amz-date".to_owned(), stamp.to_owned()),
    ]);
    if let Some(token) = credentials.get("AWS_SESSION_TOKEN") {
        ensure!(
            !token.is_empty()
                && token.len() <= 4096
                && token
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && !b"\r\n\t".contains(&b)),
            "invalid S3 session token"
        );
        headers.insert("x-amz-security-token".to_owned(), token.clone());
    }
    let signed = headers
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers = headers
        .iter()
        .map(|(key, value)| format!("{key}:{value}\n"))
        .collect::<String>();
    let canonical = format!(
        "{}\n{path}\n\n{canonical_headers}\n{signed}\n{hash}",
        method.as_str()
    );
    let day = &stamp[..8];
    let scope = format!("{day}/{region}/s3/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical.as_bytes()))
    );
    let key = hmac(format!("AWS4{secret}").as_bytes(), day.as_bytes());
    let key = hmac(&key, region.as_bytes());
    let key = hmac(&key, b"s3");
    let key = hmac(&key, b"aws4_request");
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed}, Signature={}",
        hex::encode(hmac(&key, to_sign.as_bytes()))
    );
    headers.insert("authorization".to_owned(), authorization);
    Ok(headers)
}

/// Sign an S3 path-style request (also used for RustFS's SigV4 admin API).
/// Caller configures the client with `no_proxy()`, redirects disabled and finite timeouts.
/// The endpoint must be a validated HTTPS origin or literal IPv4 loopback HTTP origin.
pub fn signed_request(
    client: &reqwest::blocking::Client,
    method: reqwest::Method,
    endpoint: &str,
    path: &str,
    region: &str,
    credentials: &BTreeMap<String, String>,
    body: &[u8],
) -> Result<reqwest::blocking::RequestBuilder> {
    let origin = storage_origin(endpoint)?;
    let host = match origin.port() {
        Some(port) => format!("{}:{port}", origin.host_str().context("missing S3 host")?),
        None => origin.host_str().context("missing S3 host")?.to_owned(),
    };
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let headers = auth_headers(&method, &host, path, region, credentials, body, &stamp)?;
    let has_body = !body.is_empty() || method == reqwest::Method::PUT;
    let mut builder = client.request(method, format!("{}{path}", endpoint.trim_end_matches('/')));
    for (key, value) in headers {
        builder = builder.header(key, value);
    }
    if has_body {
        builder = builder.body(body.to_vec());
    }
    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn creds() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("AWS_ACCESS_KEY_ID".into(), "AKIDEXAMPLE".into()),
            (
                "AWS_SECRET_ACCESS_KEY".into(),
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            ),
        ])
    }
    #[test]
    fn aws_documented_get_fixture() {
        // AWS example keys, date and S3 path; independent HMAC-SHA256 fixture
        // for the exact canonical headers this signer sends (including payload hash).
        let headers = auth_headers(
            &reqwest::Method::GET,
            "examplebucket.s3.amazonaws.com",
            "/test.txt",
            "us-east-1",
            &creds(),
            b"",
            "20130524T000000Z",
        )
        .unwrap();
        assert_eq!(headers["authorization"], "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=14f6a0997b2b70a86f4726658a6575b5109092ccb5fd328f51b369c44b4ac958");
    }
    #[test]
    fn sign_head_put_and_port() {
        let creds = BTreeMap::from([
            ("AWS_ACCESS_KEY_ID".into(), "abcdef".into()),
            ("AWS_SECRET_ACCESS_KEY".into(), "0123456789".into()),
        ]);
        let head = auth_headers(
            &reqwest::Method::HEAD,
            "127.0.0.1:9000",
            "/celld-dev",
            "us-east-1",
            &creds,
            b"",
            "20261005T010203Z",
        )
        .unwrap();
        let put = auth_headers(
            &reqwest::Method::PUT,
            "127.0.0.1:9000",
            "/rustfs/admin/v3/bucket-durability/celld-dev",
            "us-east-1",
            &creds,
            br#"{"mode":"strict"}"#,
            "20261005T010203Z",
        )
        .unwrap();
        assert_eq!(head["host"], "127.0.0.1:9000");
        assert_ne!(head["authorization"], put["authorization"]);
        assert_eq!(
            put["x-amz-content-sha256"],
            hex::encode(Sha256::digest(br#"{"mode":"strict"}"#))
        );
        assert!(put["authorization"].contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"));
        assert!(auth_headers(
            &reqwest::Method::GET,
            "127.0.0.1:9000",
            "/a?x=y",
            "us-east-1",
            &creds,
            b"",
            "20261005T010203Z"
        )
        .is_err());
    }
}
