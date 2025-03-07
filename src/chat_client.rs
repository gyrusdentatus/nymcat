// src/chat_client.rs
use crate::common::{ChatMessage, LogLevel, log};
use nym_sdk::tcp_proxy::NymProxyClient;
use nym_sdk::mixnet::{Recipient, NymNetworkDetails};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::signal;
use tokio_util::codec::{BytesCodec, FramedRead, FramedWrite};
use tokio_stream::StreamExt;
use futures_util::sink::SinkExt;
use std::net::SocketAddr;
use serde_json;

const PROXY_CLIENT_PORT: u16 = 8070;
const PROXY_CLIENT_TIMEOUT: u64 = 300; // 5 min connection timeout
const PROXY_CLIENT_POOL_SIZE: usize = 2;

pub struct ChatClient {
    username: String,
    room_address: String,
    verbosity: LogLevel,
}

impl ChatClient {
    pub fn new(username: String, room_address: String, verbosity: LogLevel) -> Self {
        Self {
            username,
            room_address,
            verbosity,
        }
    }

    pub async fn run(&self, env_path: Option<String>) -> anyhow::Result<()> {
        // Clean up the room address format if needed
        let address_str = self.room_address.strip_prefix("nym://").unwrap_or(&self.room_address);
        
        // Parse the recipient address
        let address = Recipient::try_from_base58_string(address_str)
            .map_err(|_| anyhow::anyhow!("Invalid Nym address format"))?;
        
        // Initialize network details
        let network_details = if let Some(path) = env_path {
            NymNetworkDetails::new_from_env_file(path)
        } else {
            NymNetworkDetails::new_from_env()
        };
        
        // Create the proxy client
        let proxy_client = NymProxyClient::new(
            address,
            "127.0.0.1",
            &PROXY_CLIENT_PORT.to_string(),
            PROXY_CLIENT_TIMEOUT,
            network_details,
            PROXY_CLIENT_POOL_SIZE,
        ).await?;
        
        // Clone for running the proxy
        let proxy_run = proxy_client.clone();
        
        // Start the proxy client
        tokio::spawn(async move {
            log(LogLevel::Debug, LogLevel::Debug, "Starting proxy client");
            if let Err(e) = proxy_run.run().await {
                eprintln!("Proxy client error: {}", e);
            }
        });
        
        // Wait a moment for the proxy to initialize
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        
        // Connect to the local proxy
        let stream = match TcpStream::connect(format!("127.0.0.1:{}", PROXY_CLIENT_PORT)).await {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("Failed to connect to proxy: {}", e);
                proxy_client.disconnect().await;
                return Err(e.into());
            }
        };
        
        log(LogLevel::Debug, self.verbosity, "Connected to local proxy");
        
        // Split the stream
        let (read_half, write_half) = stream.into_split();
        
        // Setup framed reading/writing
        let mut framed_read = FramedRead::new(read_half, BytesCodec::new());
        let mut framed_write = FramedWrite::new(write_half, BytesCodec::new());
        
        // Send join message
        let join_msg = ChatMessage::Join {
            username: self.username.clone(),
        };
        
        if let Ok(join_bytes) = serde_json::to_vec(&join_msg) {
            if let Err(e) = framed_write.send(bytes::Bytes::from(join_bytes)).await {
                eprintln!("Failed to send join message: {}", e);
                proxy_client.disconnect().await;
                return Err(e.into());
            }
        }
        
        println!("Joined chat room as {}", self.username);
        
        // Handle user input in a separate task
        let username = self.username.clone();
        let input_verbosity = self.verbosity;
        
        let framed_write_ref = framed_write.clone();
        let mut framed_write_clone = framed_write;
        tokio::spawn(async move {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                
                let text_msg = ChatMessage::Text {
                    from: username.clone(),
                    content: line.trim().to_string(),
                    timestamp,
                };
                
                log(LogLevel::Debug, input_verbosity, &format!("Sending text message: {}", line.trim()));
                
                match serde_json::to_vec(&text_msg) {
                    Ok(msg_bytes) => {
                        if let Err(e) = framed_write_clone.send(bytes::Bytes::from(msg_bytes)).await {
                            eprintln!("Failed to send message: {}", e);
                            break;
                        }
                    },
                    Err(e) => {
                        log(LogLevel::Debug, input_verbosity, &format!("Failed to serialize message: {}", e));
                    }
                }
            }
        });
        
        // Handle Ctrl+C for clean exit
        let username_exit = self.username.clone();
        let exit_verbosity = self.verbosity;
        let proxy_client_exit = proxy_client.clone();
        
        let mut framed_write_exit = framed_write_ref;
        tokio::spawn(async move {
            signal::ctrl_c().await.ok();
            println!("Leaving chat room...");
            log(LogLevel::Info, exit_verbosity, "Leaving chat room (Ctrl+C received)");
            
            let leave_msg = ChatMessage::Leave {
                username: username_exit.clone(),
            };
            
            if let Ok(leave_bytes) = serde_json::to_vec(&leave_msg) {
                let _ = framed_write_exit.send(bytes::Bytes::from(leave_bytes)).await;
            }
            
            // Wait briefly for the message to be sent
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            
            // Disconnect and exit
            proxy_client_exit.disconnect().await;
            std::process::exit(0);
        });
        
        // Handle incoming messages
        let username_recv = self.username.clone();
        let recv_verbosity = self.verbosity;
        
        while let Some(result) = framed_read.next().await {
            match result {
                Ok(bytes) => {
                    log(LogLevel::Trace, recv_verbosity, &format!(
                        "Received raw message: {} bytes", bytes.len()
                    ));
                    
                    if let Ok(message) = serde_json::from_slice::<ChatMessage>(&bytes) {
                        match &message {
                            ChatMessage::Join { username } if username != &username_recv => {
                                println!("User joined: {}", username);
                                log(LogLevel::Info, recv_verbosity, &format!("User joined: {}", username));
                            },
                            ChatMessage::Leave { username } if username != &username_recv => {
                                println!("User left: {}", username);
                                log(LogLevel::Info, recv_verbosity, &format!("User left: {}", username));
                            },
                            ChatMessage::Text { from, content, .. } if from != &username_recv => {
                                println!("{}: {}", from, content);
                                log(LogLevel::Info, recv_verbosity, &format!("Message from {}: {}", from, content));
                            },
                            ChatMessage::StateSync { history, participants } => {
                                log(LogLevel::Debug, recv_verbosity, &format!(
                                    "Received state sync with {} messages and {} participants",
                                    history.len(), participants.len()
                                ));
                                
                                println!("--- Current participants: {} ---", participants.join(", "));
                                
                                for item in history {
                                    if &item.from != &username_recv {
                                        println!("[HISTORY] {}: {}", item.from, item.content);
                                        log(LogLevel::Debug, recv_verbosity, &format!(
                                            "History item: {} - {}", item.from, item.content
                                        ));
                                    }
                                }
                            },
                            _ => {
                                // Ignore messages about self
                                log(LogLevel::Debug, recv_verbosity, "Ignoring message about self");
                            }
                        }
                    } else {
                        log(LogLevel::Debug, recv_verbosity, "Failed to parse incoming message");
                        if recv_verbosity == LogLevel::Trace {
                            log(LogLevel::Trace, recv_verbosity, &format!("Raw message: {:?}", bytes));
                        }
                    }
                },
                Err(e) => {
                    log(LogLevel::Debug, recv_verbosity, &format!("Error reading from stream: {}", e));
                    break;
                }
            }
        }
        
        // Disconnect before exiting
        proxy_client.disconnect().await;
        
        println!("Disconnected from chat room.");
        Ok(())
    }
}
