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
        timestamp: u64,
    },
    StateSync {
        history: Vec<HistoryItem>,
        participants: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub from: String,
    pub content: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    None,
    Info,
    Debug,
    Trace,
}

pub fn log(level: LogLevel, current_level: LogLevel, msg: &str) {
    if level as usize <= current_level as usize {
        match level {
            LogLevel::Info => println!("[INFO] {}", msg),
            LogLevel::Debug => println!("[DEBUG] {}", msg),
            LogLevel::Trace => println!("[TRACE] {}", msg),
            LogLevel::None => unreachable!(),
        }
    }
}
