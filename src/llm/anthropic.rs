use super::*;
use serde_json::{Value, json};

const VERSION: &str = "2023-06-01";

fn split_system(messages: &[Msg]) -> (String, Vec<&Msg>) {
    let mut system = String::new();
    let mut rest = Vec::with_capacity(messages.len());
    for m in messages {
        if m.role == "system" {
            if !system.is_empty() {
                system.push('\n');
            }
            system.push_str(&m.content);
        } else {
            rest.push(m);
        }
    }
    (system, rest)
}

fn build_body(model: &str, req: &ChatRequest, stream: bool) -> Value {
    let (system, rest) = split_system(&req.messages);
    let mut body = json!({
        "model": model,
        // max_tokens is mandatory on the Messages API.
        "max_tokens": req.max_tokens.unwrap_or(8192),
        "messages": rest.iter().map(|m| anthropic_message(m)).collect::<Vec<_>>(),
        "stream": stream,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    body
}

fn request(http: &Client, base: &str, key: &str, body: &Value) -> reqwest::RequestBuilder {
    http.post(format!("{base}/v1/messages"))
        .header("anthropic-version", VERSION)
        .header("x-api-key", key)
        .json(body)
}

pub fn spawn_stream(
    http: &Client,
    base: &str,
    key: &str,
    model: &str,
    req: ChatRequest,
    tx: StreamTx,
) -> tokio::task::JoinHandle<()> {
    let (http, base, key, model) = (
        http.clone(),
        base.to_string(),
        key.to_string(),
        model.to_string(),
    );
    tokio::spawn(async move {
        let resp = request(&http, &base, &key, &build_body(&model, &req, true))
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Delta::Error(format!("provider connect failed: {e}")));
                return;
            }
        };
        if !resp.status().is_success() {
            let _ = tx.send(Delta::Error(read_error_body(resp).await));
            return;
        }
        let mut parser = sse::SseParser::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if tx.is_closed() {
                return;
            }
            match chunk {
                Ok(bytes) => {
                    let batch = parser.push(&bytes);
                    for data in batch.data {
                        let Ok(v) = serde_json::from_str::<Value>(&data) else {
                            continue;
                        };
                        match v["type"].as_str() {
                            Some("content_block_delta") => {
                                if let Some(t) =
                                    v.pointer("/delta/thinking").and_then(|t| t.as_str())
                                {
                                    if !t.is_empty()
                                        && tx.send(Delta::Thought(t.to_string())).is_err()
                                    {
                                        return;
                                    }
                                }
                                if let Some(t) = v.pointer("/delta/text").and_then(|t| t.as_str()) {
                                    if !t.is_empty() && tx.send(Delta::Text(t.to_string())).is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            Some("error") => {
                                let msg = v
                                    .pointer("/error/message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("anthropic error");
                                let _ = tx.send(Delta::Error(msg.to_string()));
                                return;
                            }
                            // message_stop is the end. Don't keep reading into the close.
                            Some("message_stop") => return,
                            _ => {}
                        }
                    }
                    if batch.done {
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Delta::Error(format!(
                        "stream read failed: {}",
                        error_detail(&e)
                    )));
                    return;
                }
            }
        }
    })
}

pub async fn call(
    http: &Client,
    base: &str,
    key: &str,
    model: &str,
    system: &str,
    messages: &[Msg],
    _json_mode: bool,
    max_tokens: Option<u32>,
) -> anyhow::Result<String> {
    let mut msgs = vec![msg("system", system)];
    msgs.extend(messages.iter().cloned());
    let req = ChatRequest {
        messages: msgs,
        temperature: Some(0.0),
        max_tokens,
        json_mode: false,
    };
    let resp = request(http, base, key, &build_body(model, &req, false))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", read_error_body(resp).await);
    }
    let v: Value = resp.json().await?;
    let text = v["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if text.is_empty() {
        anyhow::bail!("anthropic returned no text: {}", v);
    }
    Ok(text)
}
