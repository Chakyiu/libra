use super::*;
use serde_json::{json, Value};

fn build_body(model: &str, req: &ChatRequest, stream: bool) -> Value {
    let mut body = json!({
        "model": model,
        "messages": req.messages,
        "stream": stream,
    });
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(m) = req.max_tokens {
        body["max_tokens"] = json!(m);
    }
    if req.json_mode {
        body["response_format"] = json!({ "type": "json_object" });
    }
    body
}

fn request(http: &Client, base: &str, key: &str, path: &str, body: &Value) -> reqwest::RequestBuilder {
    let mut req = http.post(format!("{base}/{path}")).json(body);
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    req
}

pub fn spawn_stream(http: &Client, base: &str, key: &str, model: &str, req: ChatRequest, tx: StreamTx, _ollama: bool) {
    let (http, base, key, model) = (http.clone(), base.to_string(), key.to_string(), model.to_string());
    tokio::spawn(async move {
        let body = build_body(&model, &req, true);
        let resp = request(&http, &base, &key, "chat/completions", &body).send().await;
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
            match chunk {
                Ok(bytes) => {
                    for data in parser.push(&bytes) {
                        let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
                        if let Some(err) = v.pointer("/error/message").and_then(|e| e.as_str()) {
                            let _ = tx.send(Delta::Error(err.to_string()));
                            return;
                        }
                        if let Some(text) = v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()) {
                            if !text.is_empty() {
                                let _ = tx.send(Delta::Text(text.to_string()));
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
    let resp = request(http, base, key, "chat/completions", &build_body(model, &req, false)).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", read_error_body(resp).await);
    }
    let v: Value = resp.json().await?;
    v.pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("provider returned no content: {}", v))
}

pub async fn embed(http: &Client, base: &str, key: &str, model: &str, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
    let body = json!({ "model": model, "input": texts });
    let resp = request(http, base, key, "embeddings", &body).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", read_error_body(resp).await);
    }
    let v: Value = resp.json().await?;
    let Some(data) = v["data"].as_array() else {
        anyhow::bail!("unexpected embeddings response: {}", &v.to_string()[..v.to_string().len().min(200)]);
    };
    let mut out = Vec::with_capacity(data.len());
    for d in data {
        let Some(vals) = d["embedding"].as_array() else { continue };
        out.push(vals.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect());
    }
    Ok(out)
}

/// Ollama < 0.9 has no /v1/embeddings — use the native endpoint instead.
pub async fn ollama_embed_native(http: &Client, base: &str, model: &str, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
    let root = base.trim_end_matches("/v1").trim_end_matches('/');
    let resp = http
        .post(format!("{root}/api/embed"))
        .json(&json!({ "model": model, "input": texts }))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", read_error_body(resp).await);
    }
    let v: Value = resp.json().await?;
    let Some(arr) = v["embeddings"].as_array() else {
        anyhow::bail!("unexpected ollama embed response: {v}");
    };
    Ok(arr
        .iter()
        .filter_map(|e| e.as_array().map(|vals| vals.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect()))
        .collect())
}
