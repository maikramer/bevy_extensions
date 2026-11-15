//! Protocolo do `vael-tools`: métodos BRP, cliente HTTP std-only e
//! descoberta de sessões/engines.
//!
//! O transporte é o mesmo do `bevy_remote`: JSON-RPC 2.0 sobre HTTP/1.1,
//! `POST /`, `Connection: close`. O runtime novo e a bridge legacy partilham
//! a porta por omissão (`15702`), mas usam namespaces diferentes:
//! `vael-tools/*` para as ferramentas novas e `viber.*` para compatibilidade.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Porta por omissão do bridge/engine (a mesma do `vael-tools run --bridge`).
pub const DEFAULT_TOOLS_PORT: u16 = 15702;
/// Nome do ficheiro de identidade da engine na pasta da sessão.
pub const ENGINE_FILE: &str = "engine.json";

pub const METHOD_PING: &str = "vael-tools/ping";
pub const METHOD_APP_INFO: &str = "vael-tools/app_info";
pub const METHOD_TREE: &str = "vael-tools/tree";
pub const METHOD_LOGS: &str = "vael-tools/logs";
pub const METHOD_SCREENSHOT: &str = "vael-tools/screenshot";
pub const METHOD_SCREENSHOT_STATUS: &str = "vael-tools/screenshot_status";
pub const METHOD_BURST: &str = "vael-tools/burst";
pub const METHOD_BURST_STATUS: &str = "vael-tools/burst_status";
pub const METHOD_FRAME_STATS: &str = "vael-tools/frame_stats";
pub const METHOD_HITCHES: &str = "vael-tools/hitches";
pub const METHOD_MEMORY: &str = "vael-tools/memory";
pub const METHOD_TIMINGS: &str = "vael-tools/timings";
pub const METHOD_TRACE_START: &str = "vael-tools/trace_start";
pub const METHOD_TRACE_STOP: &str = "vael-tools/trace_stop";
pub const METHOD_TRACE_STATUS: &str = "vael-tools/trace_status";
pub const METHOD_TRACE_EXPORT: &str = "vael-tools/trace_export";
pub const METHOD_MARK: &str = "vael-tools/mark";
pub const METHOD_STEP: &str = "vael-tools/step";
pub const METHOD_PLAY: &str = "vael-tools/play";
pub const METHOD_PAUSE: &str = "vael-tools/pause";
pub const METHOD_QUERY: &str = "vael-tools/query";
pub const METHOD_WATCH: &str = "vael-tools/watch";
pub const METHOD_KEY: &str = "vael-tools/input.key";
pub const METHOD_TEXT: &str = "vael-tools/input.text";
pub const METHOD_CLICK: &str = "vael-tools/input.click";
pub const METHOD_MOVE: &str = "vael-tools/input.move";
pub const METHOD_RAYCAST: &str = "vael-tools/raycast";

/// Namespace legacy (engine `viber`), usado como fallback durante a transição.
pub const LEGACY_PING: &str = "viber.ping";
pub const LEGACY_TREE: &str = "viber.tree";
pub const LEGACY_LOGS: &str = "viber.logs";
pub const LEGACY_LUA: &str = "viber.lua";
pub const LEGACY_SCREENSHOT: &str = "viber.screenshot";
pub const LEGACY_SCREENSHOT_STATUS: &str = "viber.screenshot_status";
pub const LEGACY_BURST: &str = "viber.burst";
pub const LEGACY_BURST_STATUS: &str = "viber.burst_status";
pub const LEGACY_PROFILER: &str = "viber.profiler";
pub const LEGACY_PROFILER_TAB: &str = "viber.profiler.tab";
pub const LEGACY_PROFILER_TRACE_START: &str = "viber.profiler.trace_start";
pub const LEGACY_PROFILER_TRACE_STOP: &str = "viber.profiler.trace_stop";
pub const LEGACY_PROFILER_TRACE_STATUS: &str = "viber.profiler.trace_status";
pub const LEGACY_PROFILER_TRACE_EXPORT: &str = "viber.profiler.trace_export";
pub const LEGACY_PROFILER_MARK: &str = "viber.profiler.mark";
pub const LEGACY_KEY: &str = "viber.input.key";
pub const LEGACY_TEXT: &str = "viber.input.text";
pub const LEGACY_CLICK: &str = "viber.input.click";
pub const LEGACY_MOVE: &str = "viber.input.move";

const MAX_RESPONSE: u64 = 256 << 20;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Cliente BRP mínimo, sem dependências HTTP.
pub struct BridgeClient {
    pub host: String,
    pub port: u16,
}

impl BridgeClient {
    pub fn localhost(port: u16) -> Self {
        Self {
            host: "127.0.0.1".into(),
            port,
        }
    }

    /// POST JSON-RPC e devolve `result`; um `error` BRP vira `Err`.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(method, params, READ_TIMEOUT)
    }

    fn call_timeout(&self, method: &str, params: Value, read_timeout: Duration) -> Result<Value> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_vec(&request)?;
        let mut stream = self.connect()?;
        stream.set_read_timeout(Some(read_timeout))?;
        let http = format!(
            "POST / HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.host,
            self.port,
            body.len()
        );
        stream.write_all(http.as_bytes())?;
        stream.write_all(&body)?;
        stream.flush()?;

        let mut raw = Vec::new();
        stream.take(MAX_RESPONSE).read_to_end(&mut raw)?;
        if raw.len() as u64 >= MAX_RESPONSE {
            bail!(
                "resposta do bridge excede o cap de {} MB",
                MAX_RESPONSE >> 20
            );
        }
        let (headers, response_body) = split_headers(&raw).context("resposta HTTP sem corpo")?;
        let payload = if is_chunked(headers) {
            String::from_utf8(decode_chunked(response_body)?).context("chunk inválido (UTF-8)")?
        } else {
            String::from_utf8_lossy(response_body).into_owned()
        };
        let status = http_status(headers);
        let parsed: Value = serde_json::from_str(payload.trim()).with_context(|| {
            if status == 200 {
                "resposta não é JSON-RPC".to_string()
            } else {
                format!("resposta não é JSON-RPC (bridge devolveu HTTP {status})")
            }
        })?;
        if let Some(error) = parsed.get("error") {
            bail!(
                "bridge error {}: {}",
                error.get("code").and_then(Value::as_i64).unwrap_or(0),
                error.get("message").and_then(Value::as_str).unwrap_or("?")
            );
        }
        Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
    }

    fn connect(&self) -> Result<TcpStream> {
        const ATTEMPTS: usize = 20;
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()?
            .next()
            .context("endereço do bridge não resolve")?;
        let mut last = None;
        for attempt in 0..ATTEMPTS {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(READ_TIMEOUT))?;
                    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
                    return Ok(stream);
                }
                Err(error) => {
                    last = Some(error);
                    if attempt + 1 < ATTEMPTS {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        }
        bail!(
            "a ligar ao bridge {}:{} (a engine corre com `vael-tools run --bridge`?): {}",
            self.host,
            self.port,
            last.expect("pelo menos uma tentativa")
        )
    }

    /// Ping novo com fallback para `viber.ping`.
    pub fn ping(&self) -> Result<Value> {
        match self.call_timeout(METHOD_PING, json!({}), PROBE_TIMEOUT) {
            Ok(value) => Ok(value),
            Err(_) => self.call_timeout(LEGACY_PING, json!({}), PROBE_TIMEOUT),
        }
    }

    /// Pede uma captura e faz polling até o PNG estar completo.
    pub fn screenshot(&self, timeout_ms: u64) -> Result<(Vec<u8>, String)> {
        self.screenshot_with(METHOD_SCREENSHOT, METHOD_SCREENSHOT_STATUS, timeout_ms)
    }

    /// O mesmo com o namespace legacy (`viber.screenshot*`).
    pub fn legacy_screenshot(&self, timeout_ms: u64) -> Result<(Vec<u8>, String)> {
        self.screenshot_with(LEGACY_SCREENSHOT, LEGACY_SCREENSHOT_STATUS, timeout_ms)
    }

    fn screenshot_with(
        &self,
        request_method: &str,
        status_method: &str,
        timeout_ms: u64,
    ) -> Result<(Vec<u8>, String)> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let budget = Duration::from_millis(timeout_ms)
            .min(READ_TIMEOUT)
            .max(Duration::from_millis(50));
        let request = self.call_timeout(request_method, json!({}), budget)?;
        let id = request
            .get("id")
            .and_then(Value::as_u64)
            .context("resposta sem capture id")?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timeout ({timeout_ms} ms) à espera do screenshot");
            }
            std::thread::sleep(Duration::from_millis(50).min(remaining));
            let read = deadline
                .saturating_duration_since(Instant::now())
                .min(READ_TIMEOUT)
                .max(Duration::from_millis(20));
            let status = match self.call_timeout(status_method, json!({ "id": id }), read) {
                Ok(status) => status,
                Err(error) => {
                    if Instant::now() < deadline {
                        continue;
                    }
                    return Err(error);
                }
            };
            match status.get("status").and_then(Value::as_str) {
                Some("captured") => {
                    let b64 = status
                        .get("png_base64")
                        .and_then(Value::as_str)
                        .context("captura sem png")?;
                    let png = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .context("png base64 inválido")?;
                    let path = status
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    return Ok((png, path));
                }
                Some("pending") | None => continue,
                Some(other) => bail!("captura falhou: {other}"),
            }
        }
    }

    pub fn key(&self, key: &str, state: &str, hold_ms: Option<u64>) -> Result<Value> {
        self.call(
            METHOD_KEY,
            json!({ "key": normalize_key(key), "state": state, "hold_ms": hold_ms }),
        )
    }

    /// Pede um burst — N frames seguidos compostos numa única folha — e faz
    /// polling até a folha chegar. `skip` são frames renderizados entre
    /// capturas (`0` = consecutivos). Devolve (png, caminho na engine,
    /// largura e altura da folha, status bruto).
    pub fn burst(
        &self,
        frames: u32,
        skip: u32,
        timeout_ms: u64,
    ) -> Result<(Vec<u8>, String, u32, u32, Value)> {
        self.burst_with(METHOD_BURST, METHOD_BURST_STATUS, frames, skip, timeout_ms)
    }

    /// O mesmo com o namespace legacy (`viber.burst*`), servido pelo monólito.
    pub fn legacy_burst(
        &self,
        frames: u32,
        skip: u32,
        timeout_ms: u64,
    ) -> Result<(Vec<u8>, String, u32, u32, Value)> {
        self.burst_with(LEGACY_BURST, LEGACY_BURST_STATUS, frames, skip, timeout_ms)
    }

    fn burst_with(
        &self,
        request_method: &str,
        status_method: &str,
        frames: u32,
        skip: u32,
        timeout_ms: u64,
    ) -> Result<(Vec<u8>, String, u32, u32, Value)> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let budget = Duration::from_millis(timeout_ms)
            .min(READ_TIMEOUT)
            .max(Duration::from_millis(50));
        let request = self.call_timeout(
            request_method,
            json!({ "frames": frames, "skip": skip }),
            budget,
        )?;
        let id = request
            .get("id")
            .and_then(Value::as_u64)
            .context("resposta sem burst id")?;
        let mut last_captured = 0u64;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timeout ({timeout_ms} ms) à espera do burst de {frames} frames");
            }
            std::thread::sleep(Duration::from_millis(50).min(remaining));
            let read = deadline
                .saturating_duration_since(Instant::now())
                .min(READ_TIMEOUT)
                .max(Duration::from_millis(20));
            let status = match self.call_timeout(status_method, json!({ "id": id }), read) {
                Ok(status) => status,
                Err(error) => {
                    if Instant::now() < deadline {
                        continue;
                    }
                    return Err(error);
                }
            };
            // Progresso no stderr (o stdout fica para o resultado final).
            if let Some(count) = status.get("captured").and_then(Value::as_u64)
                && count > last_captured
            {
                last_captured = count;
                eprintln!("vael-tools: burst {count}/{frames} frames capturados");
            }
            match status.get("status").and_then(Value::as_str) {
                Some("captured") => {
                    let b64 = status
                        .get("png_base64")
                        .and_then(Value::as_str)
                        .context("burst capturado sem png")?;
                    let png = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .context("png base64 inválido")?;
                    let path = status
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let sheet = status.get("sheet");
                    let dim = |key: &str| -> u32 {
                        sheet
                            .and_then(|sheet| sheet.get(key))
                            .and_then(Value::as_u64)
                            .unwrap_or(0) as u32
                    };
                    return Ok((png, path, dim("sheet_w"), dim("sheet_h"), status));
                }
                Some("capturing" | "composing" | "pending") | None => continue,
                Some("error") => {
                    let cause = status
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("desconhecida");
                    bail!("burst falhou: {cause}");
                }
                Some(other) => bail!("estado inesperado do burst: {other}"),
            }
        }
    }

    pub fn text(&self, text: &str) -> Result<Value> {
        self.call(METHOD_TEXT, json!({ "text": text }))
    }

    pub fn click(&self, x: f32, y: f32, state: &str) -> Result<Value> {
        self.call(METHOD_CLICK, json!({ "x": x, "y": y, "state": state }))
    }

    pub fn move_cursor(&self, x: f32, y: f32) -> Result<Value> {
        self.call(METHOD_MOVE, json!({ "x": x, "y": y }))
    }
}

/// `true` se houver um TCP vivo na porta (probe curto para a UI do editor).
pub fn is_listening(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

/// Identidade da engine publicada em `engine.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngineInfo {
    pub pid: u32,
    pub port: u16,
    pub world: String,
    #[serde(default)]
    pub started_at_ms: u64,
    #[serde(default)]
    pub log: String,
}

/// Caminhos de uma sessão (um mundo = uma sessão).
pub struct SessionPaths {
    base: PathBuf,
}

impl SessionPaths {
    pub fn for_world(world: &Path) -> Self {
        let world = std::fs::canonicalize(world).unwrap_or_else(|_| world.to_path_buf());
        Self::for_base(
            cache_root()
                .join("vael-tools")
                .join("sessions")
                .join(session_slug(&world)),
        )
    }

    pub fn for_base(base: PathBuf) -> Self {
        Self { base }
    }

    pub fn sessions_root() -> PathBuf {
        cache_root().join("vael-tools").join("sessions")
    }

    /// Sessões novas + legadas (`<cache>/viber/session`), sem duplicar slugs.
    pub fn all() -> Vec<(String, Self)> {
        let mut roots = vec![Self::sessions_root(), cache_root().join("viber/session")];
        roots.dedup();
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for root in roots {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(slug) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if !seen.insert(slug.clone()) {
                    continue;
                }
                out.push((slug, Self::for_base(path)));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub fn engine_file(&self) -> PathBuf {
        self.base.join(ENGINE_FILE)
    }

    pub fn engine_info(&self) -> Option<EngineInfo> {
        let raw = std::fs::read_to_string(self.engine_file()).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn write_engine(&self, info: &EngineInfo) -> Result<()> {
        std::fs::create_dir_all(&self.base)
            .with_context(|| format!("a criar {}", self.base.display()))?;
        let raw = serde_json::to_vec(info)?;
        let tmp = self.engine_file().with_extension("json.tmp");
        std::fs::write(&tmp, raw).with_context(|| format!("a escrever {}", tmp.display()))?;
        std::fs::rename(&tmp, self.engine_file())
            .with_context(|| format!("a publicar {}", self.engine_file().display()))?;
        Ok(())
    }

    pub fn clear_engine(&self) {
        let _ = std::fs::remove_file(self.engine_file());
    }
}

/// Resolve a porta a usar: `--port`, `VAEL_TOOLS_PORT`, a sessão do mundo,
/// a engine viva mais recente do registo, ou a porta por omissão.
pub fn resolve_port(explicit: Option<u16>, world: Option<&Path>) -> u16 {
    if let Some(port) = explicit {
        return port;
    }
    if let Ok(raw) = std::env::var("VAEL_TOOLS_PORT")
        && let Ok(port) = raw.trim().parse::<u16>()
    {
        return port;
    }
    let target = world.map(|world| {
        let canonicalized = std::fs::canonicalize(world).unwrap_or_else(|_| world.to_path_buf());
        canonicalized.to_string_lossy().into_owned()
    });
    let mut fallback_port: Option<u16> = None;
    for (_, paths) in SessionPaths::all() {
        let Some(info) = paths.engine_info() else {
            continue;
        };
        if let Some(target) = &target {
            let candidate =
                std::fs::canonicalize(&info.world).unwrap_or_else(|_| info.world.clone().into());
            if candidate.to_string_lossy() == *target {
                return info.port;
            }
        }
        // Engine viva mais recente: o fallback quando o mundo pedido não
        // casa com sessão nenhuma (a porta fixa 15702 é histórico do `viber
        // run`; o alocador actual dá porta própria a cada engine).
        if is_listening(info.port)
            && fallback_port.is_none_or(|current| info.started_at_ms > started_of(current))
        {
            fallback_port = Some(info.port);
        }
    }
    fallback_port.unwrap_or(DEFAULT_TOOLS_PORT)
}

/// `started_at_ms` da engine registrada na porta dada (0 se desconhecido) —
/// o desempate do fallback de [`resolve_port`].
fn started_of(port: u16) -> u64 {
    SessionPaths::all()
        .into_iter()
        .filter_map(|(_, paths)| paths.engine_info())
        .find(|info| info.port == port)
        .map(|info| info.started_at_ms)
        .unwrap_or(0)
}

fn cache_root() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".cache"))
}

/// FNV-1a 64 estável entre processos (o `DefaultHasher` é semeado à sorte).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn session_slug(world: &Path) -> String {
    let name = world
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("world");
    let parent = world
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let digest = fnv1a(world.as_os_str().as_encoded_bytes());
    format!("{parent}-{name}-{digest:016x}")
}

/// Separa headers do corpo na resposta HTTP crua.
fn split_headers(raw: &[u8]) -> Option<(&[u8], &[u8])> {
    let index = raw.windows(4).position(|window| window == b"\r\n\r\n")?;
    Some((&raw[..index], &raw[index + 4..]))
}

fn http_status(headers: &[u8]) -> u16 {
    String::from_utf8_lossy(headers)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

fn is_chunked(headers: &[u8]) -> bool {
    String::from_utf8_lossy(headers)
        .lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(name, value)| {
            name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
        })
}

fn decode_chunked(body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let Some(pos) = rest.windows(2).position(|window| window == b"\r\n") else {
            bail!("resposta chunked truncada (sem chunk terminador)");
        };
        let size_text = std::str::from_utf8(&rest[..pos])
            .map_err(|_| anyhow::anyhow!("chunk size inválido"))?
            .split(';')
            .next()
            .unwrap_or("0")
            .trim();
        let size = usize::from_str_radix(size_text, 16).context("chunk size inválido")?;
        if size == 0 {
            break;
        }
        let start = pos + 2;
        let end = start
            .checked_add(size)
            .ok_or_else(|| anyhow::anyhow!("chunk size inválido"))?;
        if end > rest.len() {
            bail!("resposta chunked truncada");
        }
        out.extend_from_slice(&rest[start..end]);
        rest = &rest[end..];
        if rest.starts_with(b"\r\n") {
            rest = &rest[2..];
        }
    }
    Ok(out)
}

/// Normaliza aliases de teclas para as variantes serde do `KeyCode`.
#[must_use]
pub fn normalize_key(raw: &str) -> String {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    let named = match lower.as_str() {
        "space" | "spacebar" => Some("Space"),
        "enter" | "return" => Some("Enter"),
        "esc" | "escape" => Some("Escape"),
        "tab" => Some("Tab"),
        "backspace" => Some("Backspace"),
        "delete" | "del" => Some("Delete"),
        "insert" => Some("Insert"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "pageup" => Some("PageUp"),
        "pagedown" => Some("PageDown"),
        "up" | "arrowup" => Some("ArrowUp"),
        "down" | "arrowdown" => Some("ArrowDown"),
        "left" | "arrowleft" => Some("ArrowLeft"),
        "right" | "arrowright" => Some("ArrowRight"),
        "shift" => Some("ShiftLeft"),
        "ctrl" | "control" => Some("ControlLeft"),
        "alt" => Some("AltLeft"),
        "capslock" => Some("CapsLock"),
        "f1" => Some("F1"),
        "f2" => Some("F2"),
        "f3" => Some("F3"),
        "f4" => Some("F4"),
        "f5" => Some("F5"),
        "f6" => Some("F6"),
        "f7" => Some("F7"),
        "f8" => Some("F8"),
        "f9" => Some("F9"),
        "f10" => Some("F10"),
        "f11" => Some("F11"),
        "f12" => Some("F12"),
        _ => None,
    };
    if let Some(name) = named {
        return name.to_string();
    }
    let mut chars = trimmed.chars();
    if let (Some(first), None) = (chars.next(), chars.next()) {
        if first.is_ascii_alphabetic() {
            return format!("Key{}", first.to_ascii_uppercase());
        }
        if let Some(digit) = first.to_digit(10) {
            return format!("Digit{digit}");
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn mock_server(response: &'static str) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            // Ler headers + corpo até ao Content-Length. Ler só um chunk podia
            // fechar o socket enquanto o cliente ainda escrevia o body e
            // provocar `Connection reset by peer` intermitente.
            let mut buf = Vec::new();
            let mut header_end = None;
            let mut body_len = 0usize;
            loop {
                if let Some(pos) = header_end
                    && buf.len() >= pos + 4 + body_len
                {
                    break;
                }
                let mut chunk = [0u8; 4096];
                let read = stream.read(&mut chunk).expect("read");
                if read == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..read]);
                if header_end.is_none()
                    && let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    body_len = String::from_utf8_lossy(&buf[..pos])
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.trim()
                                .eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    header_end = Some(pos);
                }
            }
            stream.write_all(response.as_bytes()).expect("write");
        });
        port
    }

    #[test]
    fn call_reads_a_json_rpc_result() {
        let port = mock_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 49\r\n\r\n{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"pong\":true}}",
        );
        let client = BridgeClient::localhost(port);
        let value = client.call("vael-tools/ping", json!({})).expect("call");
        assert_eq!(value["pong"], true);
    }

    #[test]
    fn call_decodes_a_chunked_body() {
        let port = mock_server(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
             31\r\n{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"pong\":true}}\r\n0\r\n\r\n",
        );
        let client = BridgeClient::localhost(port);
        let value = client.call("vael-tools/ping", json!({})).expect("call");
        assert_eq!(value["pong"], true);
    }

    #[test]
    fn call_maps_brp_errors_to_err() {
        let port = mock_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 68\r\n\r\n{\"jsonrpc\":\"2.0\",\"id\":0,\"error\":{\"code\":-32601,\"message\":\"nope\"}}",
        );
        let client = BridgeClient::localhost(port);
        let error = client.call("vael-tools/ping", json!({})).unwrap_err();
        assert!(error.to_string().contains("nope"));
    }

    #[test]
    fn normalize_key_accepts_friendly_aliases() {
        assert_eq!(normalize_key("w"), "KeyW");
        assert_eq!(normalize_key("7"), "Digit7");
        assert_eq!(normalize_key("esc"), "Escape");
        assert_eq!(normalize_key("f12"), "F12");
        assert_eq!(normalize_key("KeyW"), "KeyW");
    }

    #[test]
    fn session_engine_roundtrips_in_a_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let paths = SessionPaths::for_base(dir.path().join("session"));
        let info = EngineInfo {
            pid: 42,
            port: 15702,
            world: "/tmp/world.bsn".into(),
            started_at_ms: 1,
            log: "/tmp/engine.log".into(),
        };
        paths.write_engine(&info).unwrap();
        assert_eq!(paths.engine_info(), Some(info));
        paths.clear_engine();
        assert!(paths.engine_info().is_none());
    }
}
