// src/chat_client.rs
use crate::common::{
    ChatMessage, Colors, LogLevel, log, format_timestamp, 
    format_participants, format_nym_address, format_nym_debug_info, separator
};
use nym_sdk::tcp_proxy::NymProxyClient;
use nym_sdk::mixnet::{Recipient, NymNetworkDetails};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::signal;
use tokio_util::codec::{BytesCodec, FramedRead, FramedWrite};
use tokio_stream::StreamExt;
use futures_util::sink::SinkExt;
use std::io::{self, Write};
use std::net::SocketAddr;
use serde_json;

const PROXY_CLIENT_PORT: u16 = 8070;
const PROXY_CLIENT_TIMEOUT: u64 = 300; // 5 min connection timeout
const PROXY_CLIENT_POOL_SIZE: usize = 2;
const TERMINAL_WIDTH: usize = 80; // Default terminal width

pub struct ChatClient {
    username: String,
    room_address: String,
    verbosity: LogLevel,
    connection_time: SystemTime,
    message_count: usize,
}

impl ChatClient {
    pub fn new(username: String, room_address: String, verbosity: LogLevel) -> Self {
        Self {
            username,
            room_address,
            verbosity,
            connection_time: SystemTime::now(),
            message_count: 0,
        }
    }

    pub async fn run(&mut self, env_path: Option<String>) -> anyhow::Result<()> {
        // Clear screen and print welcome banner
        self.print_welcome_banner();
        
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
        
        // Status update
        self.print_status("Connecting to Nym network...");
        
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
                eprintln!("{}Error:{} Proxy client error: {}", Colors::RED, Colors::RESET, e);
            }
        });
        
        // Status update
        self.print_status("Waiting for proxy initialization...");
        
        // Wait a moment for the proxy to initialize
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        
        // Status update
        self.print_status("Connecting to local proxy...");
        
        // Connect to the local proxy
        let stream = match TcpStream::connect(format!("127.0.0.1:{}", PROXY_CLIENT_PORT)).await {
            Ok(stream) => stream,
            Err(e) => {
                self.print_error(&format!("Failed to connect to proxy: {}", e));
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
        
        // Status update
        self.print_status("Joining chat room...");
        
        // Send join message
        let join_msg = ChatMessage::Join {
            username: self.username.clone(),
        };
        
        if let Ok(join_bytes) = serde_json::to_vec(&join_msg) {
            if let Err(e) = framed_write.send(bytes::Bytes::from(join_bytes)).await {
                self.print_error(&format!("Failed to send join message: {}", e));
                proxy_client.disconnect().await;
                return Err(e.into());
            }
        }
        
        // Print join confirmation
        self.print_system_message(&format!("Joined chat room as {}{}{}", 
            Colors::BRIGHT_BLUE, self.username, Colors::RESET));
        
        // Print separator
        println!("{}", separator(Some("Messages"), TERMINAL_WIDTH));
        
        // Handle user input in a separate task
        let username = self.username.clone();
        let input_verbosity = self.verbosity;
        
        let framed_write_ref = framed_write.clone();
        let mut framed_write_clone = framed_write;
        
        tokio::spawn(async move {
            let stdin = BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            
            // Print the input prompt
            print!("\r{}> {}", Colors::BRIGHT_GREEN, Colors::RESET);
            io::stdout().flush().ok();
            
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    print!("\r{}> {}", Colors::BRIGHT_GREEN, Colors::RESET);
                    io::stdout().flush().ok();
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
                            eprintln!("{}Error:{} Failed to send message: {}", Colors::RED, Colors::RESET, e);
                            break;
                        }
                    },
                    Err(e) => {
                        log(LogLevel::Debug, input_verbosity, &format!("Failed to serialize message: {}", e));
                    }
                }
                
                // Format and print own message for immediate feedback
                let formatted = text_msg.format(true);
                println!("\r{}", formatted); // \r to clear the prompt
                
                // Redraw the input prompt
                print!("\r{}> {}", Colors::BRIGHT_GREEN, Colors::RESET);
                io::stdout().flush().ok();
            }
        });
        
        // Handle Ctrl+C for clean exit
        let username_exit = self.username.clone();
        let exit_verbosity = self.verbosity;
        let proxy_client_exit = proxy_client.clone();
        
        let mut framed_write_exit = framed_write_ref;
        tokio::spawn(async move {
            signal::ctrl_c().await.ok();
            println!("\r{}Leaving chat room...{}", Colors::YELLOW, Colors::RESET);
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
                                let formatted = message.format(false);
                                println!("\r{}", formatted); // \r to clear the prompt
                                self.redraw_prompt();
                                
                                log(LogLevel::Info, recv_verbosity, &format!("User joined: {}", username));
                                self.message_count += 1;
                            },
                            ChatMessage::Leave { username } if username != &username_recv => {
                                let formatted = message.format(false);
                                println!("\r{}", formatted); // \r to clear the prompt
                                self.redraw_prompt();
                                
                                log(LogLevel::Info, recv_verbosity, &format!("User left: {}", username));
                                self.message_count += 1;
                            },
                            ChatMessage::Text { from, content, .. } if from != &username_recv => {
                                let formatted = message.format(false);
                                println!("\r{}", formatted); // \r to clear the prompt
                                self.redraw_prompt();
                                
                                log(LogLevel::Info, recv_verbosity, &format!("Message from {}: {}", from, content));
                                self.message_count += 1;
                            },
                            ChatMessage::StateSync { history, participants } => {
                                log(LogLevel::Debug, recv_verbosity, &format!(
                                    "Received state sync with {} messages and {} participants",
                                    history.len(), participants.len()
                                ));
                                
                                // Print participant list with nice formatting
                                let part_header = format!("Current Participants ({})", participants.len());
                                println!("\r{}", separator(Some(&part_header), TERMINAL_WIDTH));
                                println!("{}", format_participants(participants, &username_recv));
                                println!("{}", separator(None, TERMINAL_WIDTH));
                                
                                // Print history with timestamps and colors
                                if !history.is_empty() {
                                    println!("\r{}", separator(Some("History"), TERMINAL_WIDTH));
                                    for item in history {
                                        if &item.from != &username_recv {
                                            println!("{}", item.format(false));
                                            self.message_count += 1;
                                        }
                                    }
                                    println!("{}", separator(None, TERMINAL_WIDTH));
                                }
                                
                                self.redraw_prompt();
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
                    self.print_error(&format!("Connection error: {}", e));
                    break;
                }
            }
        }
        
        // Disconnect before exiting
        proxy_client.disconnect().await;
        
        self.print_system_message("Disconnected from chat room.");
        Ok(())
    }
    
    // Helper method to redraw the prompt after printing messages
    fn redraw_prompt(&self) {
        print!("{}> {}", Colors::BRIGHT_GREEN, Colors::RESET);
        io::stdout().flush().ok();
    }
    
    // Print a system status message
    fn print_status(&self, message: &str) {
        println!("{}[STATUS]{} {}", Colors::BRIGHT_CYAN, Colors::RESET, message);
    }
    
    // Print an error message
    fn print_error(&self, message: &str) {
        eprintln!("{}[ERROR]{} {}", Colors::BRIGHT_RED, Colors::RESET, message);
    }
    
    // Print a system message
    fn print_system_message(&self, message: &str) {
        println!("{}[SYSTEM]{} {}", Colors::BRIGHT_YELLOW, Colors::RESET, message);
    }
    
    // Print welcome banner
    fn print_welcome_banner(&self) {
        // Clear screen
        print!("\x1B[2J\x1B[1;1H");
        io::stdout().flush().ok();
        
        println!("{}{}", Colors::BRIGHT_CYAN, r"
 _   _ _   _ __  __  ____    _  _____ 
| \ | \    V |  \/  |/ ___|  / \|_   _|
|  \|  \  /  | |\/| | |     / _ \ | |  
| |\   | |   | |  | | |___ / ___ \| |  
|_| \ _|_|  _|_|  |_|\____/_/   \_\_|  
        ", Colors::RESET);
        
        println!("{}{}{} A privacy-focused chat over the Nym mixnet {}{}{}\n",
            Colors::DIM,
            "•",
            Colors::RESET,
            Colors::DIM,
            "•",
            Colors::RESET
        );
        
        println!("{}Room:{} {}", 
            Colors::BRIGHT_YELLOW, 
            Colors::RESET,
            self.room_address.strip_prefix("nym://").unwrap_or(&self.room_address)
        );
        
        println!("{}Username:{} {}{}{}", 
            Colors::BRIGHT_YELLOW, 
            Colors::RESET,
            Colors::BRIGHT_BLUE,
            self.username,
            Colors::RESET
        );
        
        println!("{}Debug Level:{} {}\n", 
            Colors::BRIGHT_YELLOW, 
            Colors::RESET,
            self.verbosity
        );
        
        println!("{}", separator(Some("Connection"), TERMINAL_WIDTH));
    }
}
