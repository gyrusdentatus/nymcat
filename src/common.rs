// src/common.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChatMessage {
    Join {
        username: String,
    },
    Leave {
        username: String,
    },
    Text {
        from: String,
        content: String,
    },
}
