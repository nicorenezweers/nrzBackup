//! Minimal MCP client (JSON-RPC 2.0 over Streamable HTTP, JSON or SSE answers).

use std::time::Duration;

use serde_json::{Value, json};

pub struct McpClient<'a> {
    http: &'a reqwest::Client,
    url: &'a str,
    token: &'a str,
    session: Option<String>,
    next_id: u64,
}

impl McpClient<'_> {
    pub async fn send(&mut self, msg: Value, timeout: Duration) -> Result<Option<Value>, String> {
        let id = msg.get("id").cloned();
        let mut req = self
            .http
            .post(self.url)
            .timeout(timeout)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", "2025-06-18")
            .json(&msg);
        if !self.token.is_empty() {
            req = req.bearer_auth(self.token);
        }
        if let Some(s) = &self.session {
            req = req.header("Mcp-Session-Id", s);
        }
        let res = req
            .send()
            .await
            .map_err(|e| format!("MCP-Server {} nicht erreichbar: {e}", self.url))?;
        if let Some(s) = res
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session = Some(s.to_string());
        }
        let status = res.status();
        let sse = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/event-stream"));
        let text = res.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "MCP HTTP {status}: {}",
                text.chars().take(300).collect::<String>()
            ));
        }
        let Some(id) = id else { return Ok(None) };
        let messages: Vec<Value> = if sse {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str(d.trim()).ok())
                .collect()
        } else if text.trim().is_empty() {
            vec![]
        } else {
            vec![serde_json::from_str(&text).map_err(|e| format!("MCP-Antwort unlesbar: {e}"))?]
        };
        let msg = messages
            .into_iter()
            .flat_map(|m| match m {
                Value::Array(a) => a,
                m => vec![m],
            })
            .find(|m| m.get("id") == Some(&id))
            .ok_or("Keine Antwort vom MCP-Server")?;
        if let Some(err) = msg.get("error") {
            return Err(err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("MCP-Fehler")
                .to_string());
        }
        Ok(Some(msg.get("result").cloned().unwrap_or(Value::Null)))
    }

    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.next_id += 1;
        let msg =
            json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params });
        Ok(self.send(msg, timeout).await?.unwrap_or(Value::Null))
    }
}

impl<'a> McpClient<'a> {
    pub fn new(http: &'a reqwest::Client, url: &'a str, token: &'a str) -> Self {
        McpClient {
            http,
            url,
            token,
            session: None,
            next_id: 0,
        }
    }

    /// `initialize` + `notifications/initialized`. Optional for stateless servers, so failures are ignored.
    pub async fn initialize(&mut self) {
        let init = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "nrzbackup", "title": "nrzBackup", "version": crate::VERSION },
                }),
                Duration::from_secs(30),
            )
            .await;
        if init.is_ok() {
            let _ = self
                .send(
                    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                    Duration::from_secs(30),
                )
                .await;
        }
    }

    /// Calls a tool; returns the structured content or the text parsed as JSON (or as a string).
    pub async fn call_tool(
        &mut self,
        name: &str,
        args: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": args }),
                timeout,
            )
            .await?;
        let text: String = result
            .get("content")
            .and_then(Value::as_array)
            .map(|c| {
                c.iter()
                    .filter_map(|x| x.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(if text.is_empty() {
                format!("{name} fehlgeschlagen")
            } else {
                text
            });
        }
        if let Some(v) = result.get("structuredContent").filter(|v| !v.is_null()) {
            return Ok(v.clone());
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}
