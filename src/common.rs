// src/common.rs
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Local};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Color codes for terminal output
pub struct Colors;

impl Colors {
    pub const RESET: &'static str = "\x1b[0m";
    pub const BOLD: &'static str = "\x1b[1m";
    pub const DIM: &'static str = "\x1b[2m";
    pub const ITALIC: &'static str = "\x1b[3m";
    pub const UNDERLINE: &'static str = "\x1b[4m";
    
    // Foreground colors
    pub const BLACK: &'static str = "\x1b[30m";
    pub const RED: &'static str = "\x1b[31m";
    pub const GREEN: &'static str = "\x1b[32m";
    pub const YELLOW: &'static str = "\x1b[33m";
    pub const BLUE: &'static str = "\x1b[34m";
    pub const MAGENTA: &'static str = "\x1b[35m";
    pub const CYAN: &'static str = "\x1b[36m";
    pub const WHITE: &'static str = "\x1b[37m";
    
    // Bright foreground colors
    pub const BRIGHT_BLACK: &'static str = "\x1b[90m";
    pub const BRIGHT_RED: &'static str = "\x1b[91m";
    pub const BRIGHT_GREEN: &'static str = "\x1b[92m";
    pub const BRIGHT_YELLOW: &'static str = "\x1b[93m";
    pub const BRIGHT_BLUE: &'static str = "\x1b[94m";
    pub const BRIGHT_MAGENTA: &'static str = "\x1b[95m";
    pub const BRIGHT_CYAN: &'static str = "\x1b[96m";
    pub const BRIGHT_WHITE: &'static str = "\x1b[97m";
    
    // Background colors
    pub const BG_BLACK: &'static str = "\x1b[40m";
    pub const BG_RED: &'static str = "\x1b[41m";
    pub const BG_GREEN: &'static str = "\x1b[42m";
    pub const BG_YELLOW: &'static str = "\x1b[43m";
    pub const BG_BLUE: &'static str = "\x1b[44m";
    pub const BG_MAGENTA: &'static str = "\x1b[45m";
    pub const BG_CYAN: &'static str = "\x1b[46m";
    pub const BG_WHITE: &'static str = "\x1b[47m";
}

/// Chat messages exchanged between clients and server
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

impl ChatMessage {
    /// Returns a formatted string representation with colors and timestamps
    pub fn format(&self, is_self: bool) -> String {
        match self {
            ChatMessage::Join { username } => {
                format!(
                    "{}{}{} {} joined the room{}",
                    Colors::DIM,
                    format_timestamp(SystemTime::now()),
                    Colors::RESET,
                    format!("{}{}{}",
                        Colors::BRIGHT_GREEN,
                        username,
                        Colors::RESET
                    ),
                    format!(" {}{}{}", 
                        Colors::DIM, 
                        "👋", 
                        Colors::RESET
                    )
                )
            },
            ChatMessage::Leave { username } => {
                format!(
                    "{}{}{} {} left the room{}",
                    Colors::DIM,
                    format_timestamp(SystemTime::now()),
                    Colors::RESET,
                    format!("{}{}{}",
                        Colors::BRIGHT_YELLOW,
                        username,
                        Colors::RESET
                    ),
                    format!(" {}{}{}", 
                        Colors::DIM, 
                        "👋", 
                        Colors::RESET
                    )
                )
            },
            ChatMessage::Text { from, content, timestamp } => {
                let time_str = format_timestamp_from_unix(*timestamp);
                let name_color = if is_self {
                    Colors::BRIGHT_BLUE
                } else {
                    get_username_color(from)
                };
                
                format!(
                    "{}{}{} {}{}{}: {}",
                    Colors::DIM,
                    time_str,
                    Colors::RESET,
                    name_color,
                    from,
                    Colors::RESET,
                    content
                )
            },
            ChatMessage::StateSync { .. } => {
                format!(
                    "{}{}{}  State synchronization received",
                    Colors::DIM,
                    format_timestamp(SystemTime::now()),
                    Colors::RESET
                )
            },
        }
    }
}

/// History item for storing chat history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub from: String,
    pub content: String,
    pub timestamp: u64,
}

impl HistoryItem {
    /// Format a history item with timestamp and colors
    pub fn format(&self, is_self: bool) -> String {
        let time_str = format_timestamp_from_unix(self.timestamp);
        let name_color = if is_self {
            Colors::BRIGHT_BLUE
        } else {
            get_username_color(&self.from)
        };
        
        format!(
            "{}{}{} [HISTORY] {}{}{}: {}",
            Colors::DIM,
            time_str,
            Colors::RESET,
            name_color,
            self.from,
            Colors::RESET,
            self.content
        )
    }
}

/// Log levels for debugging
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    None,
    Info,
    Debug,
    Trace,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogLevel::None => write!(f, "NONE"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Trace => write!(f, "TRACE"),
        }
    }
}

/// Enhanced logging function with timestamps and colors
pub fn log(level: LogLevel, current_level: LogLevel, msg: &str) {
    if level as usize <= current_level as usize {
        let now = Local::now();
        let timestamp = now.format("%H:%M:%S%.3f").to_string();
        
        match level {
            LogLevel::Info => println!(
                "{}[{}]{} {}[{}]{} {}",
                Colors::DIM, timestamp, Colors::RESET,
                Colors::GREEN, level, Colors::RESET,
                msg
            ),
            LogLevel::Debug => println!(
                "{}[{}]{} {}[{}]{} {}",
                Colors::DIM, timestamp, Colors::RESET,
                Colors::YELLOW, level, Colors::RESET,
                msg
            ),
            LogLevel::Trace => println!(
                "{}[{}]{} {}[{}]{} {}",
                Colors::DIM, timestamp, Colors::RESET,
                Colors::MAGENTA, level, Colors::RESET,
                msg
            ),
            LogLevel::None => unreachable!(),
        }
    }
}

/// Format a system time to a readable timestamp
pub fn format_timestamp(time: SystemTime) -> String {
    let datetime: DateTime<Local> = time.into();
    datetime.format("%H:%M:%S").to_string()
}

/// Format a unix timestamp (seconds since epoch) to a readable time
pub fn format_timestamp_from_unix(timestamp: u64) -> String {
    let system_time = UNIX_EPOCH + std::time::Duration::from_secs(timestamp);
    format_timestamp(system_time)
}

/// Get a consistent color for a username
pub fn get_username_color(username: &str) -> &'static str {
    // Simple hash function to determine color
    let hash = username.bytes().fold(0u32, |acc, byte| acc.wrapping_add(byte as u32));
    
    // Select from a set of distinct, readable colors
    match hash % 6 {
        0 => Colors::BRIGHT_RED,
        1 => Colors::BRIGHT_GREEN,
        2 => Colors::BRIGHT_YELLOW,
        3 => Colors::BRIGHT_CYAN,
        4 => Colors::BRIGHT_MAGENTA,
        5 => Colors::BRIGHT_BLUE,
        _ => unreachable!(),
    }
}

/// Generates a horizontal separator line with optional title
pub fn separator(title: Option<&str>, width: usize) -> String {
    let line_char = "─";
    
    match title {
        Some(text) => {
            let text_len = text.len();
            let remaining = width.saturating_sub(text_len + 2); // +2 for spaces
            let left = remaining / 2;
            let right = remaining - left;
            
            format!(
                "{}{}{}{}{}{}{}",
                Colors::DIM,
                line_char.repeat(left),
                Colors::RESET,
                format!(" {} ", text),
                Colors::DIM,
                line_char.repeat(right),
                Colors::RESET
            )
        },
        None => {
            format!(
                "{}{}{}",
                Colors::DIM,
                line_char.repeat(width),
                Colors::RESET
            )
        }
    }
}

/// Returns a formatted list of participants
pub fn format_participants(participants: &[String], username: &str) -> String {
    if participants.is_empty() {
        return format!("{}No other participants{}", Colors::DIM, Colors::RESET);
    }
    
    let parts: Vec<String> = participants
        .iter()
        .map(|name| {
            if name == username {
                format!("{}{}{}", Colors::BRIGHT_BLUE, name, Colors::RESET)
            } else {
                format!("{}{}{}", get_username_color(name), name, Colors::RESET)
            }
        })
        .collect();
    
    parts.join(", ")
}

/// Format a Nym address for display
pub fn format_nym_address(address: &str) -> String {
    if address.len() <= 12 {
        return address.to_string();
    }
    
    let prefix = &address[0..6];
    let suffix = &address[address.len() - 6..];
    
    format!("{}{}...{}", Colors::DIM, prefix, suffix)
}

/// Truncate a string if it's too long
pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[0..max_len.saturating_sub(3)])
    }
}

/// Format Nym-specific debug information
pub fn format_nym_debug_info(sender_tag: Option<&str>, surbs: Option<u32>) -> String {
    let mut parts = Vec::new();
    
    if let Some(tag) = sender_tag {
        parts.push(format!("tag={}{}{}", Colors::CYAN, truncate_str(tag, 8), Colors::RESET));
    }
    
    if let Some(surb_count) = surbs {
        parts.push(format!("surbs={}{}{}", Colors::YELLOW, surb_count, Colors::RESET));
    }
    
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(", "))
    }
}
