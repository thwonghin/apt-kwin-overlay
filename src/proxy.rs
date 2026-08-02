use std::collections::HashMap;
use std::net::TcpStream;

use crate::server::write_response;

// Same allowlist as main/src/proxy.ts — this is how the renderer reaches
// poe.ninja/trade-site data without a CORS-related dead end.
const PROXY_HOSTS: &[&str] = &[
    "www.pathofexile.com",
    "ru.pathofexile.com",
    "pathofexile.tw",
    "poe.kakaogames.com",
    "poe.ninja",
    "www.poeprices.info",
];

const STRIPPED_REQUEST_HEADERS: &[&str] = &["host", "origin", "content-length"];

pub fn handle(
    stream: &mut TcpStream,
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    body: &[u8],
) -> std::io::Result<()> {
    let rest = path.strip_prefix("/proxy/").unwrap_or("");
    let host = rest.split('/').next().unwrap_or("");

    if !PROXY_HOSTS.contains(&host) {
        return write_response(stream, 403, "text/plain", b"host not allowed");
    }

    let url = format!("https://{rest}");
    let agent = ureq::Agent::new_with_defaults();
    let forwarded_headers: Vec<(&str, &str)> = headers
        .iter()
        .filter(|(key, _)| !key.starts_with("sec-") && !STRIPPED_REQUEST_HEADERS.contains(&key.as_str()))
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let result = if method == "POST" {
        let mut builder = agent.post(&url);
        for (key, value) in &forwarded_headers {
            builder = builder.header(*key, *value);
        }
        builder.send(body)
    } else {
        let mut builder = agent.get(&url);
        for (key, value) in &forwarded_headers {
            builder = builder.header(*key, *value);
        }
        builder.call()
    };

    match result {
        Ok(mut response) => {
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let bytes = response.body_mut().read_to_vec().unwrap_or_default();
            write_response(stream, status, &content_type, &bytes)
        }
        Err(err) => {
            eprintln!("[proxy] request to {host} failed: {err}");
            write_response(stream, 502, "text/plain", b"proxy error")
        }
    }
}
