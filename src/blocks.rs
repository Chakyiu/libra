//! Blocks are the unit of both the wire protocol and persistence: the client renders
//! them as they stream in, and the same array is stored so a chat replays without
//! re-generating anything. Shape follows upstream Vane's `blocks` table.

use crate::search::types::SearchResult;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockType {
    References,
    Related,
    Images,
    Videos,
    Widget,
    Weather,
    Stocks,
    Calculation,
    Time,
    Document,
    Message,
    Answered,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    #[serde(rename = "type")]
    pub block_type: BlockType,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

impl Block {
    pub fn references(results: Vec<SearchResult>) -> Self {
        Self {
            block_type: BlockType::References,
            message: "references".into(),
            data: Some(json!({ "web": results })),
            extra: None,
        }
    }

    pub fn related(questions: Vec<String>) -> Self {
        Self {
            block_type: BlockType::Related,
            message: "related".into(),
            data: Some(json!({ "questions": questions })),
            extra: None,
        }
    }

    pub fn images(results: Vec<SearchResult>) -> Self {
        Self {
            block_type: BlockType::Images,
            message: "images".into(),
            data: Some(json!({ "results": results })),
            extra: None,
        }
    }

    pub fn videos(results: Vec<SearchResult>) -> Self {
        Self {
            block_type: BlockType::Videos,
            message: "videos".into(),
            data: Some(json!({ "results": results })),
            extra: None,
        }
    }

    pub fn message(text: &str) -> Self {
        Self {
            block_type: BlockType::Message,
            message: text.to_string(),
            data: None,
            extra: None,
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(json!({}))
    }
}
