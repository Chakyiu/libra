use super::*;
use serde_json::{json, Value};

fn contents(messages: &[Msg]) -> (Option<Value>, Vec<Value>) {
    let mut system: Option<String> = None;
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        match m.role.as_str() {
            "system" => system = Some(m.content.clone()),
            role => out.push(json!({
                "role": if role == "assistant" { "model" } else { "user" },
                "parts": [{ "text": m.content }],
            })),
        }
    }
    (system.map(|s| json!({ "parts": [{ "text": s }] })), out)
}

fn build_body(req: &ChatRequest, stream: bool) -> Value {
    let (system, items) = contents(&req.messages);
    let mut cfg = json!({});
    if let Some(t) = req.temperature {
        cfg["temperature"] = json!(t);
    }
    if let Some(m) = req.max_tokens {
        cfg["maxOutputTokens"] = json!(m);
    }
    if req.json_mode {
        cfg["responseMimeType"] = json!("application/json");
    }
    let mut body = json!({ "contents": items, "generationConfig": cfg });
    if let Some(s) = system {
        body["systemInstruction"] = s;
    }
    let _ = stream;
    body
}

fn url(base: &str, key: &str, model: &str, method: &str) -> String {
    format!(
        "{base}/v1beta/models/{model}:{method}?key={}",
        urlencoded(key)
    )
}

fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn spawn_stream(http: &Client, base: &str, key: &str, model: &str, req: ChatRequest, tx: StreamTx) {
    let (http, base, key, model) = (http.clone(), base.to_string(), key.to_string(), model.to_string());
    tokio::spawn(async move {
        let u = format!("{}&alt=sse", url(&base, &key, &model, "streamGenerateContent"));
        let resp = match http.post(&u).json(&build_body(&req, true)).send().await {
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
            match chunk {
                Ok(bytes) => {
                    for data in parser.push(&bytes) {
                        let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
                        if let Some(err) = v.pointer("/error/message").and_then(|e| e.as_str()) {
                            let _ = tx.send(Delta::Error(err.to_string()));
                            return;
                        }
                        if let Some(parts) = v.pointer("/candidates/0/content/parts").and_then(|p| p.as_array()) {
                            for p in parts {
                                if let Some(t) = p["text"].as_str() {
                                    if !t.is_empty() {
                                        let _ = tx.send(Delta::Text(t.to_string()));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Delta::Error(format!("stream read failed: {e}")));
                    return;
                }
            }
        }
    });
}

pub async fn call(http: &Client, base: &str, key: &str, model: &str, system: &str, messages: &[Msg], json_mode: bool, max_tokens: Option<u32>) -> anyhow::Result<String> {
    let mut msgs = vec![msg("system", system)];
    msgs.extend(messages.iter().cloned());
    let req = ChatRequest {
        messages: msgs,
        temperature: Some(0.0),
        max_tokens,
        json_mode,
    };
    let resp = http
        .post(url(base, key, model, "generateContent"))
        .json(&build_body(&req, false))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", read_error_body(resp).await);
    }
    let v: Value = resp.json().await?;
    let text = v
        .pointer("/candidates/0/content/parts")
        .and_then(|p| p.as_array())
        .map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""))
        .unwrap_or_default();
    if text.is_empty() {
        anyhow::bail!("gemini returned no text: {}", v);
    }
    Ok(text)
}

pub async fn embed(http: &Client, base: &str, key: &str, model: &str, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
    let body = json!({
        "requests": texts.iter().map(|t| json!({
            "model": format!("models/{model}"),
            "content": { "parts": [{ "text": t }] },
            "outputType": "float_value",
        })).collect::<Vec<_>>()
    });
    let resp = http.post(url(base, key, model, "batchEmbedContents")).json(&body).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", read_error_body(resp).await);
    }
    let v: Value = resp.json().await?;
    let arr = v["embeddings"].as_array().cloned().unwrap_or_default();
    Ok(arr
        .iter()
        .filter_map(|e| e["values"].as_array().map(|vals| vals.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect()))
        .collect())
}
