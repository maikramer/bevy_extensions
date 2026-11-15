//! Cliente mínimo do bridge do jogo (`bevy_ext_bridge_runtime`/`jackdaw_remote`).
//!
//! O painel fala com o jogo a correr como o `vael-tools debug` fala: JSON-RPC 2.0
//! em HTTP sobre TCP em `127.0.0.1:15702` (ou `VAEL_TOOLS_PORT`). Sem
//! depender do `vael-tools`: os checkouts ficam independentes e o protocolo
//! é pequeno. Só corre quando o utilizador carrega num botão (bloqueia a
//! thread do editor), por isso não há threads nem polling.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{Value, json};

/// Porta histórica do runtime de ferramentas do jogo.
pub const DEFAULT_PORT: u16 = 15702;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const READ_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_RESPONSE: u64 = 4 << 20;

/// Porta do bridge (`VAEL_TOOLS_PORT`, por omissão 15702).
pub fn port() -> u16 {
    std::env::var("VAEL_TOOLS_PORT")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Chama um método e devolve o `result`; um `error` JSON-RPC vira `Err`.
pub fn call(method: &str, params: Value) -> Result<Value, String> {
    let port = port();
    let addr = ("127.0.0.1", port)
        .to_socket_addrs()
        .map_err(|error| format!("endereço do bridge: {error}"))?
        .next()
        .ok_or_else(|| "endereço do bridge não resolve".to_string())?;
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|error| {
        format!("sem bridge na porta {port} (o jogo está a correr com o Play?): {error}")
    })?;
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));

    let request = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": method,
        "params": params,
    });
    let body = serde_json::to_vec(&request).map_err(|error| format!("pedido: {error}"))?;
    let head = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&body))
        .and_then(|()| stream.flush())
        .map_err(|error| format!("envio do pedido: {error}"))?;

    let mut raw = Vec::new();
    stream
        .take(MAX_RESPONSE)
        .read_to_end(&mut raw)
        .map_err(|error| format!("resposta do bridge: {error}"))?;
    if raw.len() as u64 >= MAX_RESPONSE {
        return Err("resposta do bridge acima do limite".to_string());
    }
    let text = String::from_utf8_lossy(&raw);
    let (headers, payload) = split_http(&text)?;
    let payload = if is_chunked(headers) {
        decode_chunked(payload)?
    } else {
        payload.to_string()
    };
    let parsed: Value = serde_json::from_str(payload.trim())
        .map_err(|error| format!("resposta não é JSON-RPC: {error}"))?;
    if let Some(error) = parsed.get("error") {
        return Err(format!(
            "bridge: {} ({})",
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            error.get("code").and_then(Value::as_i64).unwrap_or(0)
        ));
    }
    Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
}

/// Separa cabeçalhos e corpo (o bridge fecha a ligação por pedido).
fn split_http(text: &str) -> Result<(&str, &str), String> {
    text.split_once("\r\n\r\n")
        .ok_or_else(|| "resposta HTTP sem cabeçalhos".to_string())
}

fn is_chunked(headers: &str) -> bool {
    headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        })
    })
}

/// Descodifica `Transfer-Encoding: chunked` (tamanho em hex + CRLF).
fn decode_chunked(body: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = body;
    loop {
        let (size, tail) = rest
            .split_once("\r\n")
            .ok_or_else(|| "chunk sem tamanho".to_string())?;
        let size = usize::from_str_radix(size.trim(), 16)
            .map_err(|error| format!("chunk com tamanho inválido: {error}"))?;
        if size == 0 {
            return Ok(out);
        }
        if tail.len() < size {
            return Err("chunk truncado".to_string());
        }
        out.push_str(&tail[..size]);
        rest = tail[size..].strip_prefix("\r\n").unwrap_or(&tail[size..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpo_com_content_length() {
        let text = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        let (headers, body) = split_http(text).unwrap();
        assert!(!is_chunked(headers));
        assert_eq!(body, "{}");
    }

    #[test]
    fn corpo_chunked() {
        let text = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";
        let (headers, body) = split_http(text).unwrap();
        assert!(is_chunked(headers));
        assert_eq!(decode_chunked(body).unwrap(), "{}");
    }
}
