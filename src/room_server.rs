// src/room_server.rs
use crate::common::{ChatMessage, HistoryItem, LogLevel, log};
use nym_sdk::tcp_proxy::NymProxyServer;
use nym_sdk::mixnet::NymNetworkDetails;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::signal;
use tokio_util::codec::{BytesCodec, FramedRead, FramedWrite};
use tokio_stream::StreamExt;
use futures_util::sink::SinkExt;
use serde_json;

const MAX_HISTORY_ITEMS: usize = 100;

pub struct RoomServer {
    state: Arc<Mutex<RoomState>>,
    verbosity: LogLevel,
}

struct RoomState {
    participants: HashMap<String, String>, // username -> connection_id
    history: Vec<HistoryItem>,
    connections: HashMap<String, tokio::sync::mpsc::UnboundedSender<Vec<u8>>>, // connection_id -> sender
}

impl RoomState {
    fn new() -> Self {
        Self {
            participants: HashMap::new(),
            history: Vec::new(),
            connections: HashMap::new(),
        }
    }

    fn add_history_item(&mut self, item: HistoryItem) {
        self.history.push(item);
        if self.history.len() > MAX_HISTORY_ITEMS {
            self.history.remove(0);
        }
    }
}

impl RoomServer {
    pub fn new(verbosity: LogLevel) -> Self {
        Self {
            state: Arc::new(Mutex::new(RoomState::new())),
            verbosity,
        }
    }

    pub async fn run(&self, env_path: Option<String>) -> anyhow::Result<()> {
        // Initialize network details for Nym
        let network_details = if let Some(path) = env_path {
            NymNetworkDetails::new_from_env_file(path)
        } else {
            NymNetworkDetails::new_from_env()
        };

        // Initialize the proxy server (listen on localhost:9000)
        let proxy_server = NymProxyServer::new("0.0.0.0:9000", network_details).await?;
        
        // Get the server's address for display
        let nym_address = proxy_server.nym_address().to_string();
        println!("Room created. Address: nym://{}", nym_address);
        log(LogLevel::Info, self.verbosity, &format!("Room running at nym://{}", nym_address));

        // Clone for the connection handler
        let state = Arc::clone(&self.state);
        let verbosity = self.verbosity;
        
        // Start the server
        let server_handle = tokio::spawn(async move {
            proxy_server.run().await
        });
        
        // Handle TCP connections on localhost:9000
        let listener = tokio::net::TcpListener::bind("0.0.0.0:9000").await?;
        
        // Spawn a task to accept connections
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let conn_state = Arc::clone(&state);
                        let conn_verbosity = verbosity;
                        
                        tokio::spawn(async move {
                            log(LogLevel::Debug, conn_verbosity, "New connection received");
                    
                    // Split TCP stream
                    let (read_half, write_half) = stream.into_split();
                    
                    // Setup framed reading/writing
                    let mut framed_read = FramedRead::new(read_half, BytesCodec::new());
                    let framed_write = FramedWrite::new(write_half, BytesCodec::new());
                    
                    // Create a channel for sending messages to this client
                    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
                    
                    // Generate a unique connection ID
                    let connection_id = uuid::Uuid::new_v4().to_string();
                    
                    // Store the sender in our state
                    {
                        let mut state = conn_state.lock().unwrap();
                        state.connections.insert(connection_id.clone(), sender);
                    }
                    
                    // Spawn a task to handle sending messages to this client
                    let conn_id_clone = connection_id.clone();
                    let writer_state = Arc::clone(&conn_state);
                    let writer_verbosity = conn_verbosity;
                    
                    let mut framed_write = framed_write;
                    tokio::spawn(async move {
                        while let Some(message) = receiver.recv().await {
                            log(LogLevel::Trace, writer_verbosity, &format!(
                                "Sending {} bytes to connection {}", message.len(), conn_id_clone
                            ));
                            
                            if let Err(e) = framed_write.send(bytes::Bytes::from(message)).await {
                                log(LogLevel::Debug, writer_verbosity, &format!(
                                    "Error sending to client: {}", e
                                ));
                                break;
                            }
                        }
                        
                        // Clean up the connection when the sender is dropped
                        let mut state = writer_state.lock().unwrap();
                        state.connections.remove(&conn_id_clone);
                        
                        // Remove any participants using this connection
                        let usernames: Vec<String> = state.participants
                            .iter()
                            .filter(|(_, conn_id)| **conn_id == conn_id_clone)
                            .map(|(username, _)| username.clone())
                            .collect();
                        
                        for username in usernames {
                            state.participants.remove(&username);
                            
                            // Notify others that user left
                            let leave_msg = ChatMessage::Leave {
                                username: username.clone(),
                            };
                            
                            if let Ok(leave_bytes) = serde_json::to_vec(&leave_msg) {
                                Self::broadcast(&state.connections, &leave_bytes, Some(&conn_id_clone));
                            }
                        }
                    });
                    
                    // Handle incoming messages
                    while let Some(Ok(bytes)) = framed_read.next().await {
                        log(LogLevel::Trace, conn_verbosity, &format!(
                            "Received {} bytes from connection {}", bytes.len(), connection_id
                        ));
                        
                        // Try to parse the message
                        match serde_json::from_slice::<ChatMessage>(&bytes) {
                            Ok(message) => {
                                let mut state = conn_state.lock().unwrap();
                                
                                match &message {
                                    ChatMessage::Join { username } => {
                                        log(LogLevel::Info, conn_verbosity, &format!(
                                            "User joined: {}", username
                                        ));
                                        
                                        // Store participant
                                        state.participants.insert(username.clone(), connection_id.clone());
                                        
                                        // Broadcast join message
                                        if let Ok(join_bytes) = serde_json::to_vec(&message) {
                                            Self::broadcast(&state.connections, &join_bytes, None);
                                        }
                                        
                                        // Send state sync to the new user
                                        let sync_msg = ChatMessage::StateSync {
                                            history: state.history.clone(),
                                            participants: state.participants.keys().cloned().collect(),
                                        };
                                        
                                        if let Ok(sync_bytes) = serde_json::to_vec(&sync_msg) {
                                            if let Some(sender) = state.connections.get(&connection_id) {
                                                let _ = sender.send(sync_bytes);
                                            }
                                        }
                                    },
                                    ChatMessage::Leave { username } => {
                                        log(LogLevel::Info, conn_verbosity, &format!(
                                            "User left: {}", username
                                        ));
                                        
                                        // Remove participant
                                        state.participants.remove(username);
                                        
                                        // Broadcast leave message
                                        if let Ok(leave_bytes) = serde_json::to_vec(&message) {
                                            Self::broadcast(&state.connections, &leave_bytes, None);
                                        }
                                    },
                                    ChatMessage::Text { from, content, timestamp } => {
                                        log(LogLevel::Info, conn_verbosity, &format!(
                                            "Message from {}: {}", from, content
                                        ));
                                        
                                        // Store in history
                                        let history_item = HistoryItem {
                                            from: from.clone(),
                                            content: content.clone(),
                                            timestamp: *timestamp,
                                        };
                                        
                                        state.add_history_item(history_item);
                                        
                                        // Broadcast message
                                        if let Ok(text_bytes) = serde_json::to_vec(&message) {
                                            Self::broadcast(&state.connections, &text_bytes, None);
                                        }
                                    },
                                    ChatMessage::StateSync { .. } => {
                                        // Ignore state sync requests from clients
                                        log(LogLevel::Debug, conn_verbosity, "Ignoring StateSync from client");
                                    }
                                }
                            },
                            Err(e) => {
                                log(LogLevel::Debug, conn_verbosity, &format!(
                                    "Failed to parse message: {}", e
                                ));
                            }
                        }
                    }
                    
                            log(LogLevel::Debug, conn_verbosity, &format!(
                                "Connection {} closed", connection_id
                            ));
                        });
                    },
                    Err(e) => {
                        eprintln!("Failed to accept connection: {}", e);
                    }
                }
            }
        });
        
        // Wait for Ctrl+C
        signal::ctrl_c().await?;
        println!("Shutting down room server...");
        
        Ok(())
    }
    
    fn broadcast(
        connections: &HashMap<String, tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
        message: &[u8],
        exclude: Option<&String>,
    ) {
        for (conn_id, sender) in connections {
            if let Some(excluded) = exclude {
                if conn_id == excluded {
                    continue;
                }
            }
            
            let _ = sender.send(message.to_vec());
        }
    }
}
