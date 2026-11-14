//! Redis and Upstash REST access for one group document.
//!
//! Passwords stay in the endpoint value and must not be copied into errors.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub enum Endpoint {
    Rest { base: String, token: String },
    Redis { addr: String, password: Option<String> },
}

pub fn parse(url: &str, password: &str) -> Result<Endpoint> {
    let url = url.trim();
    if url.is_empty() {
        bail!("先填写 Redis 地址");
    }
    if let Some(rest) = url.strip_prefix("https://") {
        let host = rest.split(['/', '?']).next().unwrap_or(rest);
        let token = nonempty(password).context("Upstash 需要填写 REST token")?;
        return Ok(Endpoint::Rest {
            base: format!("https://{host}"),
            token,
        });
    }
    if url.starts_with("http://") {
        bail!("同步地址要用 https，或 redis://");
    }
    if let Some(rest) = url.strip_prefix("rediss://") {
        let (userinfo, hostport) = split_authority(rest);
        let host = hostport.split([':', '/']).next().unwrap_or(hostport);
        if host.ends_with(".upstash.io") {
            let token = first_secret(userinfo, password).context("Upstash 地址里没有 token")?;
            return Ok(Endpoint::Rest {
                base: format!("https://{host}"),
                token,
            });
        }
        bail!("这条 rediss 地址请改用 Upstash 的 https:// 地址，或使用 redis://");
    }
    if let Some(rest) = url.strip_prefix("redis://") {
        let (userinfo, hostport) = split_authority(rest);
        let addr = hostport
            .split('/')
            .next()
            .unwrap_or(hostport)
            .trim()
            .to_string();
        if !addr.contains(':') {
            bail!("redis 地址需要主机和端口，例如 redis://127.0.0.1:6379");
        }
        return Ok(Endpoint::Redis {
            addr,
            password: first_secret(userinfo, password),
        });
    }
    if url.contains(':') && !url.contains("://") {
        return Ok(Endpoint::Redis {
            addr: url.to_string(),
            password: nonempty(password),
        });
    }
    bail!("地址格式用 https://、rediss://，或 redis://主机:端口");
}

pub fn display_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let Some((_, host)) = rest.split_once('@') else {
        return url.to_string();
    };
    format!("{scheme}://{host}")
}

pub async fn get_text(endpoint: &Endpoint, key: &str) -> Result<Option<String>> {
    match endpoint {
        Endpoint::Rest { base, token } => rest_command(base, token, vec!["GET".into(), key.into()])
            .await
            .map(empty_to_none),
        Endpoint::Redis { addr, password } => {
            let addr = addr.clone();
            let password = password.clone();
            let key = key.to_string();
            tokio::task::spawn_blocking(move || redis_text(&addr, password.as_deref(), &["GET", &key]))
                .await
                .context("redis 任务中断")?
                .map(empty_to_none)
        }
    }
}

pub async fn list_text(endpoint: &Endpoint, pattern: &str) -> Result<Vec<String>> {
    match endpoint {
        Endpoint::Rest { base, token } => {
            let value = rest_result(base, token, vec!["KEYS".into(), pattern.into()]).await?;
            json_string_list(&value)
        }
        Endpoint::Redis { addr, password } => {
            let addr = addr.clone();
            let password = password.clone();
            let pattern = pattern.to_string();
            tokio::task::spawn_blocking(move || {
                redis_list(&addr, password.as_deref(), &["KEYS", &pattern])
            })
            .await
            .context("redis 任务中断")?
        }
    }
}

pub async fn set_text(endpoint: &Endpoint, key: &str, value: &str) -> Result<()> {
    match endpoint {
        Endpoint::Rest { base, token } => {
            rest_command(
                base,
                token,
                vec!["SET".into(), key.into(), value.into()],
            )
            .await?;
            Ok(())
        }
        Endpoint::Redis { addr, password } => {
            let addr = addr.clone();
            let password = password.clone();
            let key = key.to_string();
            let value = value.to_string();
            tokio::task::spawn_blocking(move || {
                redis_text(&addr, password.as_deref(), &["SET", &key, &value])
            })
            .await
            .context("redis 任务中断")??;
            Ok(())
        }
    }
}

pub async fn delete_text(endpoint: &Endpoint, key: &str) -> Result<()> {
    match endpoint {
        Endpoint::Rest { base, token } => {
            rest_command(base, token, vec!["DEL".into(), key.into()]).await?;
            Ok(())
        }
        Endpoint::Redis { addr, password } => {
            let addr = addr.clone();
            let password = password.clone();
            let key = key.to_string();
            tokio::task::spawn_blocking(move || redis_text(&addr, password.as_deref(), &["DEL", &key]))
                .await
                .context("redis 任务中断")??;
            Ok(())
        }
    }
}

fn empty_to_none(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() { None } else { Some(value.to_string()) }
}

fn first_secret(userinfo: Option<&str>, password: &str) -> Option<String> {
    if let Some(secret) = nonempty(password) {
        return Some(secret);
    }
    let userinfo = userinfo?;
    let secret = userinfo.split_once(':').map(|(_, pass)| pass).unwrap_or("");
    nonempty(secret)
}

fn split_authority(rest: &str) -> (Option<&str>, &str) {
    match rest.split_once('@') {
        Some((userinfo, host)) => (Some(userinfo), host),
        None => (None, rest),
    }
}

async fn rest_command(base: &str, token: &str, args: Vec<String>) -> Result<String> {
    json_text(&rest_result(base, token, args).await?)
}

async fn rest_result(base: &str, token: &str, args: Vec<String>) -> Result<serde_json::Value> {
    let client = crate::auth::build_http_client()?;
    let response = client
        .post(base)
        .bearer_auth(token)
        .json(&args)
        .send()
        .await
        .context("连接 Upstash 失败")?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.context("Upstash 响应无法解析")?;
    if let Some(error) = body.get("error").and_then(|value| value.as_str()) {
        bail!("Upstash 拒绝了这次操作 ({status}): {error}");
    }
    if !status.is_success() {
        bail!("Upstash 返回 HTTP {status}");
    }
    Ok(body.get("result").cloned().unwrap_or(serde_json::Value::Null))
}

fn json_text(value: &serde_json::Value) -> Result<String> {
    match value {
        serde_json::Value::Null => Ok(String::new()),
        serde_json::Value::String(text) => Ok(text.clone()),
        other => Ok(other.to_string()),
    }
}

fn json_string_list(value: &serde_json::Value) -> Result<Vec<String>> {
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(items) => Ok(items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect()),
        serde_json::Value::String(text) if text.is_empty() => Ok(Vec::new()),
        _ => bail!("无法识别 redis 响应"),
    }
}

enum RedisReply {
    Text(String),
    List(Vec<String>),
}

fn redis_text(addr: &str, password: Option<&str>, args: &[&str]) -> Result<String> {
    match redis_command(addr, password, args)? {
        RedisReply::Text(text) => Ok(text),
        RedisReply::List(_) => bail!("无法识别 redis 响应"),
    }
}

fn redis_list(addr: &str, password: Option<&str>, args: &[&str]) -> Result<Vec<String>> {
    match redis_command(addr, password, args)? {
        RedisReply::List(items) => Ok(items),
        RedisReply::Text(text) if text.is_empty() => Ok(Vec::new()),
        RedisReply::Text(_) => bail!("无法识别 redis 响应"),
    }
}

fn redis_command(addr: &str, password: Option<&str>, args: &[&str]) -> Result<RedisReply> {
    let socket = addr
        .to_socket_addrs()
        .with_context(|| format!("redis 地址无效: {addr}"))?
        .next()
        .with_context(|| format!("redis 地址无法解析: {addr}"))?;
    let mut stream = TcpStream::connect_timeout(&socket, Duration::from_secs(5))
        .with_context(|| format!("连接 redis {addr} 失败"))?;
    stream.set_read_timeout(Some(Duration::from_secs(8)))?;
    stream.set_write_timeout(Some(Duration::from_secs(8)))?;
    if let Some(password) = password {
        let reply = send(&mut stream, &["AUTH", password])?;
        if let RedisReply::Text(text) = reply
            && (text.starts_with("ERR") || text.starts_with("WRONGPASS"))
        {
            bail!("redis 密码不正确");
        }
    }
    send(&mut stream, args)
}

fn send(stream: &mut TcpStream, args: &[&str]) -> Result<RedisReply> {
    let mut payload = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        payload.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        payload.extend_from_slice(arg.as_bytes());
        payload.extend_from_slice(b"\r\n");
    }
    stream.write_all(&payload).context("写入 redis 失败")?;
    read_reply(stream)
}

fn read_reply(stream: &mut impl Read) -> Result<RedisReply> {
    let line = read_line(stream)?;
    let prefix = line.as_bytes().first().copied().context("redis 响应为空")?;
    let rest = &line[1..];
    match prefix {
        b'+' | b':' => Ok(RedisReply::Text(rest.to_string())),
        b'-' => bail!("redis 返回错误"),
        b'$' => Ok(RedisReply::Text(read_bulk(stream, rest)?)),
        b'*' => {
            let count: isize = rest.parse().context("redis 数组长度无效")?;
            if count < 0 {
                return Ok(RedisReply::List(Vec::new()));
            }
            let mut items = Vec::with_capacity(count as usize);
            for _ in 0..count {
                match read_reply(stream)? {
                    RedisReply::Text(text) => items.push(text),
                    RedisReply::List(_) => bail!("无法识别 redis 响应"),
                }
            }
            Ok(RedisReply::List(items))
        }
        _ => bail!("无法识别 redis 响应"),
    }
}

fn read_bulk(stream: &mut impl Read, rest: &str) -> Result<String> {
    let len: isize = rest.parse().context("redis bulk 长度无效")?;
    if len < 0 {
        return Ok(String::new());
    }
    let mut body = vec![0_u8; len as usize];
    stream.read_exact(&mut body).context("读取 redis 数据失败")?;
    let mut crlf = [0_u8; 2];
    stream.read_exact(&mut crlf)?;
    String::from_utf8(body).context("redis 数据不是 UTF-8")
}

fn read_line(stream: &mut impl Read) -> Result<String> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        stream.read_exact(&mut byte).context("读取 redis 响应失败")?;
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            line.push(byte[0]);
        }
        if line.len() > 1024 * 1024 {
            bail!("redis 响应过长");
        }
    }
    Ok(String::from_utf8(line).context("redis 响应不是 UTF-8")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstash_rediss_url_uses_rest_and_embedded_token() {
        let endpoint = parse("rediss://default:secret@example.upstash.io:6379", "").unwrap();
        match endpoint {
            Endpoint::Rest { base, token } => {
                assert_eq!(base, "https://example.upstash.io");
                assert_eq!(token, "secret");
            }
            Endpoint::Redis { .. } => panic!("expected rest"),
        }
    }

    #[test]
    fn explicit_password_overrides_url_secret() {
        let endpoint = parse("redis://:old@127.0.0.1:6379", "new").unwrap();
        match endpoint {
            Endpoint::Redis { addr, password } => {
                assert_eq!(addr, "127.0.0.1:6379");
                assert_eq!(password.as_deref(), Some("new"));
            }
            Endpoint::Rest { .. } => panic!("expected redis"),
        }
    }

    #[test]
    fn display_hides_userinfo() {
        assert_eq!(
            display_url("rediss://default:secret@example.upstash.io:6379"),
            "rediss://example.upstash.io:6379"
        );
    }

    #[test]
    fn keys_reply_lists_bulk_strings() {
        let raw = b"*2\r\n$16\r\ncs:v1:group/home\r\n$17\r\ncs:v1:group/other\r\n";
        let reply = read_reply(&mut &raw[..]).unwrap();
        match reply {
            RedisReply::List(items) => {
                assert_eq!(items, ["cs:v1:group/home", "cs:v1:group/other"]);
            }
            RedisReply::Text(_) => panic!("expected a list"),
        }
    }

    #[test]
    fn json_keys_result_keeps_only_strings() {
        let value = serde_json::json!(["cs:v1:group/home", 1, "cs:v1:group/other"]);
        assert_eq!(
            json_string_list(&value).unwrap(),
            ["cs:v1:group/home", "cs:v1:group/other"]
        );
    }
}
