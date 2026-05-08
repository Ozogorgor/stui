//! Raw MPD protocol client over TCP.
//!
//! Implements just enough of the MPD protocol to drive playback:
//! connect, authenticate, send commands, parse key-value responses.
//!
//! MPD protocol basics:
//!   - Server sends `OK MPD <version>\n` on connect.
//!   - Each command is sent as a single line.
//!   - Responses end with `OK\n` (success) or `ACK [code] {cmd} msg\n` (error).
//!   - `idle [subsystems]` blocks until a subsystem changes, then responds
//!     with `changed: <subsystem>\n` lines followed by `OK\n`.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::TcpStream;

/// A single active TCP connection to an MPD server.
pub struct MpdConnection {
    writer: tokio::io::WriteHalf<TcpStream>,
    reader: Lines<BufReader<tokio::io::ReadHalf<TcpStream>>>,
    pub version: String,
}

impl MpdConnection {
    /// Open a new connection and perform the greeting + optional auth.
    pub async fn connect(host: &str, port: u16, password: Option<&str>) -> Result<Self> {
        let stream = TcpStream::connect(format!("{host}:{port}"))
            .await
            .with_context(|| format!("cannot connect to MPD at {host}:{port}"))?;

        let (rx, tx) = tokio::io::split(stream);
        let mut conn = MpdConnection {
            writer: tx,
            reader: BufReader::new(rx).lines(),
            version: String::new(),
        };

        // Read banner: "OK MPD <version>"
        let banner = conn
            .reader
            .next_line()
            .await?
            .context("MPD closed connection immediately")?;
        if let Some(v) = banner.strip_prefix("OK MPD ") {
            conn.version = v.to_string();
        } else {
            bail!("unexpected MPD banner: {banner}");
        }

        // Authenticate if a password is set.
        if let Some(pw) = password.filter(|p| !p.is_empty()) {
            conn.run_command(&format!("password {pw}")).await?;
        }

        Ok(conn)
    }

    /// Send a command and collect the response into a `HashMap<key, value>`.
    /// Lines of the form `key: value` are parsed; `OK` terminates.
    pub async fn command_kv(&mut self, cmd: &str) -> Result<HashMap<String, String>> {
        self.send_line(cmd).await?;
        let mut map = HashMap::new();
        loop {
            let line = self.next_line().await?;
            if line == "OK" {
                break;
            }
            if line.starts_with("ACK") {
                bail!("MPD error: {line}");
            }
            if let Some((k, v)) = line.split_once(": ") {
                map.insert(k.to_string(), v.to_string());
            }
        }
        Ok(map)
    }

    /// Send a command and wait for `OK` (no interesting response body).
    pub async fn run_command(&mut self, cmd: &str) -> Result<()> {
        self.send_line(cmd).await?;
        loop {
            let line = self.next_line().await?;
            if line == "OK" {
                return Ok(());
            }
            if line.starts_with("ACK") {
                bail!("MPD error on `{cmd}`: {line}");
            }
        }
    }

    /// Send the `idle [subsystems]` command and return the list of changed
    /// subsystems once MPD unblocks.
    pub async fn idle(&mut self, subsystems: &[&str]) -> Result<Vec<String>> {
        let cmd = if subsystems.is_empty() {
            "idle".to_string()
        } else {
            format!("idle {}", subsystems.join(" "))
        };
        self.send_line(&cmd).await?;

        let mut changed = vec![];
        loop {
            let line = self.next_line().await?;
            if line == "OK" {
                break;
            }
            if line.starts_with("ACK") {
                bail!("MPD idle error: {line}");
            }
            if let Some(sub) = line.strip_prefix("changed: ") {
                changed.push(sub.to_string());
            }
        }
        Ok(changed)
    }

    /// Send a command and return the full stream of `(key, value)` pairs
    /// in wire order.  Useful for responses where record boundaries are
    /// marked by any of several keys (e.g. `lsinfo` emits `directory:`,
    /// `file:`, or `playlist:` as a new-record marker).
    pub async fn command_kv_ordered(&mut self, cmd: &str) -> Result<Vec<(String, String)>> {
        self.send_line(cmd).await?;
        let mut out = Vec::new();
        loop {
            let line = self.next_line().await?;
            if line == "OK" {
                break;
            }
            if line.starts_with("ACK") {
                bail!("MPD error on `{cmd}`: {line}");
            }
            if let Some((k, v)) = line.split_once(": ") {
                out.push((k.to_string(), v.to_string()));
            }
        }
        Ok(out)
    }

    /// Send a command that returns multiple records (like `outputs`).
    /// Records are separated by a repeated `outputid:` / `Id:` key.
    pub async fn command_records(
        &mut self,
        cmd: &str,
        split_key: &str,
    ) -> Result<Vec<HashMap<String, String>>> {
        self.send_line(cmd).await?;
        let mut records: Vec<HashMap<String, String>> = vec![];
        let mut current: HashMap<String, String> = HashMap::new();
        loop {
            let line = self.next_line().await?;
            if line == "OK" {
                if !current.is_empty() {
                    records.push(current);
                }
                break;
            }
            if line.starts_with("ACK") {
                bail!("MPD error on `{cmd}`: {line}");
            }
            if let Some((k, v)) = line.split_once(": ") {
                if k == split_key && !current.is_empty() {
                    records.push(std::mem::take(&mut current));
                }
                current.insert(k.to_string(), v.to_string());
            }
        }
        Ok(records)
    }

    async fn send_line(&mut self, line: &str) -> Result<()> {
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn next_line(&mut self) -> Result<String> {
        self.reader
            .next_line()
            .await?
            .context("MPD connection closed unexpectedly")
    }

    /// Trigger MPD's `update` command, optionally scoped to a subpath.
    /// Returns the job ID string (from `updating_db:`) — empty if MPD
    /// didn't return one. Fire-and-forget: MPD scans in the background.
    pub async fn update_library(&mut self, subpath: Option<&str>) -> Result<String> {
        let cmd = match subpath {
            Some(p) if !p.is_empty() => {
                let mut s = String::from("update ");
                s.push('"');
                for ch in p.chars() {
                    if ch == '\\' || ch == '"' {
                        s.push('\\');
                    }
                    s.push(ch);
                }
                s.push('"');
                s
            }
            _ => "update".to_string(),
        };
        let map = self.command_kv(&cmd).await?;
        Ok(map.get("updating_db").cloned().unwrap_or_default())
    }
}
