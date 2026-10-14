use super::*;
use serde_json::{Value, json};

fn build_body(model: &str, req: &ChatRequest, stream: bool) -> Value {
    let mut body = json!({
        "model": model,
        "messages": req.messages.iter().map(openai_message).collect::<Vec<_>>(),
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
        // Reasoning models spend max_tokens on the trace. The JSON is then cut off
        // with finish_reason=length. These calls only need the object.
        body["chat_template_kwargs"] = json!({ "enable_thinking": false });
        body["reasoning"] = json!({ "effort": "none" });
    }
    body
}

/// Drop `max_tokens` when the server's context cannot hold the requested length.
fn drop_max_tokens(body: &mut Value) -> bool {
    body.as_object_mut()
        .and_then(|o| o.remove("max_tokens"))
        .is_some()
}

/// Drop the thinking-off flags when a server rejects them as unknown fields.
fn clear_reasoning_controls(body: &mut Value) -> bool {
    let Some(obj) = body.as_object_mut() else {
        return false;
    };
    let had = obj.contains_key("reasoning") || obj.contains_key("chat_template_kwargs");
    obj.remove("reasoning");
    obj.remove("chat_template_kwargs");
    had
}

/// Reasoning arrives beside `content` under a few different keys.
fn thought_delta(v: &Value) -> Option<String> {
    let delta = v.pointer("/choices/0/delta")?;
    for key in ["reasoning", "reasoning_content"] {
        if let Some(s) = delta.get(key).and_then(|c| c.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    let details = delta.get("reasoning_details")?.as_array()?;
    let mut out = String::new();
    for d in details {
        if let Some(t) = d.get("text").and_then(|t| t.as_str()) {
            out.push_str(t);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn request(
    http: &Client,
    base: &str,
    key: &str,
    path: &str,
    body: &Value,
) -> reqwest::RequestBuilder {
    let mut req = http.post(format!("{base}/{path}")).json(body);
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    req
}

pub fn spawn_stream(
    http: &Client,
    base: &str,
    key: &str,
    model: &str,
    req: ChatRequest,
    tx: StreamTx,
    _ollama: bool,
) -> tokio::task::JoinHandle<()> {
    let (http, base, key, model) = (
        http.clone(),
        base.to_string(),
        key.to_string(),
        model.to_string(),
    );
    tokio::spawn(async move {
        let mut body = build_body(&model, &req, true);
        let resp = request(&http, &base, &key, "chat/completions", &body)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Delta::Error(format!("provider connect failed: {e}")));
                return;
            }
        };
        let resp = if resp.status().is_success() {
            resp
        } else if resp.status().as_u16() != 400 {
            let _ = tx.send(Delta::Error(read_error_body(resp).await));
            return;
        } else {
            // A 400 is often an unknown field, or max_tokens larger than the context.
            // Drop one of those and try again before failing the answer.
            let mut retry = resp;
            if clear_reasoning_controls(&mut body) {
                match request(&http, &base, &key, "chat/completions", &body)
                    .send()
                    .await
                {
                    Ok(next) => retry = next,
                    Err(e) => {
                        let _ = tx.send(Delta::Error(format!("provider connect failed: {e}")));
                        return;
                    }
                }
            }
            if !retry.status().is_success()
                && retry.status().as_u16() == 400
                && drop_max_tokens(&mut body)
            {
                match request(&http, &base, &key, "chat/completions", &body)
                    .send()
                    .await
                {
                    Ok(next) => retry = next,
                    Err(e) => {
                        let _ = tx.send(Delta::Error(format!("provider connect failed: {e}")));
                        return;
                    }
                }
            }
            if !retry.status().is_success() {
                let _ = tx.send(Delta::Error(read_error_body(retry).await));
                return;
            }
            retry
        };
        let mut parser = sse::SseParser::new();
        let mut stream = resp.bytes_stream();
        let mut completed = false;
        let mut truncated = false;
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
                        if let Some(err) = v.pointer("/error/message").and_then(|e| e.as_str()) {
                            let _ = tx.send(Delta::Error(err.to_string()));
                            return;
                        }
                        if let Some(reason) = v
                            .pointer("/choices/0/finish_reason")
                            .and_then(|c| c.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            completed = true;
                            if reason == "length" {
                                truncated = true;
                            }
                        }
                        if let Some(text) = thought_delta(&v) {
                            if tx.send(Delta::Thought(text)).is_err() {
                                return;
                            }
                        }
                        if let Some(text) = v
                            .pointer("/choices/0/delta/content")
                            .and_then(|c| c.as_str())
                        {
                            if !text.is_empty() && tx.send(Delta::Text(text.to_string())).is_err() {
                                return;
                            }
                        }
                    }
                    // [DONE] is a clean end. Stop so the socket close after it
                    // is not reported as a failed body.
                    if batch.done || truncated {
                        if truncated {
                            let _ = tx.send(Delta::Truncated);
                        }
                        return;
                    }
                }
                Err(e) => {
                    if completed {
                        return;
                    }
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
    json_mode: bool,
    max_tokens: Option<u32>,
) -> anyhow::Result<String> {
    let mut msgs = vec![msg("system", system)];
    msgs.extend(messages.iter().cloned());
    let req = ChatRequest {
        messages: msgs,
        temperature: Some(0.0),
        max_tokens,
        json_mode,
    };
    let mut body = build_body(model, &req, false);
    let resp = request(http, base, key, "chat/completions", &body)
        .send()
        .await?;
    let v: Value = if !resp.status().is_success() {
        let status = resp.status();
        let err = read_error_body(resp).await;
        if status.as_u16() == 400 && clear_reasoning_controls(&mut body) {
            let retry = request(http, base, key, "chat/completions", &body)
                .send()
                .await?;
            if !retry.status().is_success() {
                anyhow::bail!("{}", read_error_body(retry).await);
            }
            retry.json().await?
        } else {
            anyhow::bail!("{err}");
        }
    } else {
        resp.json().await?
    };
    message_text(&v).ok_or_else(|| {
        let reason = v
            .pointer("/choices/0/finish_reason")
            .and_then(|c| c.as_str())
            .unwrap_or("unknown");
        anyhow::anyhow!("provider returned no content (finish_reason={reason})")
    })
}

/// Answer text, or the reasoning trace when a router left `content` null.
fn message_text(v: &Value) -> Option<String> {
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(s) = content {
        return Some(s.to_string());
    }
    for key in ["reasoning", "reasoning_content"] {
        if let Some(s) = v
            .pointer(&format!("/choices/0/message/{key}"))
            .and_then(|c| c.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(s.to_string());
        }
    }
    None
}

pub async fn embed(
    http: &Client,
    base: &str,
    key: &str,
    model: &str,
    texts: &[String],
) -> anyhow::Result<Vec<Vec<f32>>> {
    let body = json!({ "model": model, "input": texts });
    let resp = request(http, base, key, "embeddings", &body).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", read_error_body(resp).await);
    }
    let v: Value = resp.json().await?;
    let Some(data) = v["data"].as_array() else {
        anyhow::bail!(
            "unexpected embeddings response: {}",
            &v.to_string()[..v.to_string().len().min(200)]
        );
    };
    let mut out = Vec::with_capacity(data.len());
    for d in data {
        let Some(vals) = d["embedding"].as_array() else {
            continue;
        };
        out.push(
            vals.iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect(),
        );
    }
    Ok(out)
}

/// Ollama < 0.9 has no /v1/embeddings — use the native endpoint instead.
pub async fn ollama_embed_native(
    http: &Client,
    base: &str,
    model: &str,
    texts: &[String],
) -> anyhow::Result<Vec<Vec<f32>>> {
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
        .filter_map(|e| {
            e.as_array().map(|vals| {
                vals.iter()
                    .filter_map(|x| x.as_f64().map(|f| f as f32))
                    .collect()
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::message_text;
    use serde_json::json;

    #[test]
    fn reads_content_before_reasoning() {
        let v = json!({"choices":[{"message":{"content":" {\"queries\":[\"poe build\"]} ","reasoning":"thinking"}}]});
        assert!(message_text(&v).unwrap().contains("poe build"));
    }

    #[test]
    fn falls_back_to_reasoning_when_content_is_null() {
        let v = json!({"choices":[{"finish_reason":"length","message":{"content":null,"reasoning":"{\"queries\":[\"poe\"]}"}}]});
        assert_eq!(message_text(&v).as_deref(), Some("{\"queries\":[\"poe\"]}"));
    }
}
