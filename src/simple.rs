// src/simple.rs
use crate::common::{ChatMessage, HistoryItem, LogLevel, log};
use nym_sdk::mixnet::{MixnetClient, MixnetMessageSender, Recipient, IncludedSurbs, AnonymousSenderTag};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal;
use std::time::{SystemTime, UNIX_EPOCH, Duration};

const SURBS_PER_MESSAGE: u32 = 50;

#[derive(Default)]
struct RoomState {
    participants: HashMap<String, AnonymousSenderTag>,
    history: Vec<HistoryItem>,
}

pub async fn run_room_server(verbosity: LogLevel, env_file: Option<String>) -> anyhow::Result<()> {
    // Set environment if provided
    if let Some(path) = &env_file {
        std::env::set_var("NYM_ENV_FILE", path);
    }
    
    // Create a mixnet client
    let mut client = MixnetClient::connect_new().await?;
    let room_address = *client.nym_address();
    let room_address_str = client.nym_address().to_string();
    
    println!("Room created. Address: nym://{}", room_address_str);
    log(LogLevel::Info, verbosity, &format!("Room address: {}", room_address_str));
    
    // Create shared state
    let state = Arc::new(Mutex::new(RoomState::default()));
    let state_clone = Arc::clone(&state);
    let sender = client.split_sender();
    
    // Handle messages
    let msg_verbosity = verbosity;
    client.on_messages(move |msg| {
        log(LogLevel::Trace, msg_verbosity, &format!("Received raw message: {} bytes", msg.message.len()));
        
        // Try to parse the message
        let message: ChatMessage = match serde_json::from_slice(&msg.message) {
            Ok(m) => m,
            Err(e) => {
                log(LogLevel::Debug, msg_verbosity, &format!("Failed to parse message: {}", e));
                return;
            }
        };
        
        match &message {
            ChatMessage::Join { username } => {
                if let Some(sender_tag) = msg.sender_tag {
                    println!("User joined: {}", username);
                    log(LogLevel::Info, msg_verbosity, &format!("User joined: {} with sender tag", username));
                    
                    // Store participant
                    {
                        let mut state_lock = state_clone.lock().unwrap();
                        state_lock.participants.insert(username.clone(), sender_tag);
                    }
                    
                    // Send state sync to new user
                    let state_data = {
                        let state_lock = state_clone.lock().unwrap();
                        (
                            state_lock.history.clone(),
                            state_lock.participants.keys().cloned().collect::<Vec<_>>()
                        )
                    };
                    
                    let (history, participants) = state_data;
                    let sync_msg = ChatMessage::StateSync {
                        history,
                        participants,
                    };
                    
                    if let Ok(sync_bytes) = serde_json::to_vec(&sync_msg) {
                        let sender_clone = sender.clone();
                        tokio::spawn(async move {
                            if let Err(e) = sender_clone.send_reply(sender_tag, &sync_bytes).await {
                                eprintln!("Failed to send sync: {}", e);
                            }
                        });
                    }
                    
                    // Broadcast join to others
                    broadcast_to_participants(&message, &state_clone, &sender, username, msg_verbosity);
                }
            },
            ChatMessage::Leave { username } => {
                println!("User left: {}", username);
                log(LogLevel::Info, msg_verbosity, &format!("User left: {}", username));
                
                // Remove participant
                {
                    let mut state_lock = state_clone.lock().unwrap();
                    state_lock.participants.remove(username);
                }
                
                // Broadcast leave to others
                broadcast_to_participants(&message, &state_clone, &sender, username, msg_verbosity);
            },
            ChatMessage::Text { from, content, timestamp } => {
                println!("{}: {}", from, content);
                log(LogLevel::Info, msg_verbosity, &format!("Message from {}: {}", from, content));
                
                // Store in history
                {
                    let mut state_lock = state_clone.lock().unwrap();
                    let history_item = HistoryItem {
                        from: from.clone(),
                        content: content.clone(),
                        timestamp: *timestamp,
                    };
                    state_lock.history.push(history_item);
                }
                
                // Broadcast message to others
                broadcast_to_participants(&message, &state_clone, &sender, from, msg_verbosity);
            },
            ChatMessage::StateSync { .. } => {
                log(LogLevel::Debug, msg_verbosity, "Ignoring StateSync message at server");
            }
        }
    }).await;
    
    // Wait for Ctrl+C
    signal::ctrl_c().await?;
    println!("Shutting down room server...");
    
    Ok(())
}

fn broadcast_to_participants(
    message: &ChatMessage,
    state: &Arc<Mutex<RoomState>>,
    sender: &nym_sdk::mixnet::MixnetClientSender,
    skip_username: &str,
    verbosity: LogLevel,
) {
    // Get participants to broadcast to
    let tags = {
        let state_lock = state.lock().unwrap();
        state_lock.participants.iter()
            .filter(|(name, _)| *name != skip_username)
            .map(|(_, tag)| *tag)
            .collect::<Vec<_>>()
    };
    
    if tags.is_empty() {
        return;
    }
    
    // Broadcast the message
    if let Ok(msg_bytes) = serde_json::to_vec(message) {
        for tag in tags {
            let sender_clone = sender.clone();
            let msg_bytes_clone = msg_bytes.clone();
            
            tokio::spawn(async move {
                if let Err(e) = sender_clone.send_reply(tag, &msg_bytes_clone).await {
                    eprintln!("Failed to broadcast: {}", e);
                }
            });
        }
    }
}

pub async fn run_chat_client(username: String, room_address: String, verbosity: LogLevel, env_file: Option<String>) -> anyhow::Result<()> {
    // Set environment if provided
    if let Some(path) = &env_file {
        std::env::set_var("NYM_ENV_FILE", path);
    }
    
    // Clean up address
    let address_str = room_address.strip_prefix("nym://").unwrap_or(&room_address);
    let room_address = Recipient::from_str(address_str)?;
    
    // Create mixnet client
    let mut client = MixnetClient::connect_new().await?;
    log(LogLevel::Info, verbosity, &format!("Connected to mixnet as {}", client.nym_address()));
    
    let sender = client.split_sender();
    
    // Send join message
    let join_msg = ChatMessage::Join { username: username.clone() };
    log(LogLevel::Debug, verbosity, "Sending join message");
    sender.send_message(room_address, &serde_json::to_vec(&join_msg)?, IncludedSurbs::Amount(SURBS_PER_MESSAGE)).await?;
    
    println!("Joined chat room as {}", username);
    
    // Handle user input
    let username_input = username.clone();
    let sender_input = sender.clone();
    let input_verbosity = verbosity;
    let input_room_address = room_address;
    
    tokio::spawn(async move {
        let stdin = BufReader::new(tokio::io::stdin());
        let mut lines = stdin.lines();
        
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() { continue; }
            
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            
            let text_msg = ChatMessage::Text {
                from: username_input.clone(),
                content: line.trim().to_string(),
                timestamp,
            };
            
            log(LogLevel::Debug, input_verbosity, &format!("Sending text message: {}", line.trim()));
            match serde_json::to_vec(&text_msg) {
                Ok(msg_bytes) => {
                    if let Err(e) = sender_input.send_message(
                        input_room_address,
                        &msg_bytes,
                        IncludedSurbs::Amount(SURBS_PER_MESSAGE)
                    ).await {
                        eprintln!("Failed to send message: {}", e);
                    }
                },
                Err(e) => {
                    log(LogLevel::Debug, input_verbosity, &format!("Failed to serialize message: {}", e));
                }
            }
        }
    });
    
    // Handle Ctrl+C for clean exit
    let username_exit = username.clone();
    let sender_exit = sender.clone();
    let exit_verbosity = verbosity;
    let exit_room_address = room_address;
    
    tokio::spawn(async move {
        signal::ctrl_c().await.ok();
        println!("Leaving chat room...");
        log(LogLevel::Info, exit_verbosity, "Leaving chat room (Ctrl+C received)");
        
        let leave_msg = ChatMessage::Leave { username: username_exit };
        match serde_json::to_vec(&leave_msg) {
            Ok(msg_bytes) => {
                sender_exit.send_message(
                    exit_room_address,
                    &msg_bytes,
                    IncludedSurbs::Amount(SURBS_PER_MESSAGE)
                ).await.ok();
            },
            Err(e) => {
                log(LogLevel::Debug, exit_verbosity, &format!("Failed to serialize leave message: {}", e));
            }
        }
        
        // Wait briefly for message to be sent
        tokio::time::sleep(Duration::from_millis(500)).await;
        std::process::exit(0);
    });
    
    // Handle incoming messages
    let username_msgs = username.clone();
    let msgs_verbosity = verbosity;
    
    client.on_messages(move |msg| {
        log(LogLevel::Trace, msgs_verbosity, &format!("Received raw message: {} bytes", msg.message.len()));
        
        if let Ok(message) = serde_json::from_slice::<ChatMessage>(&msg.message) {
            match &message {
                ChatMessage::Join { username: join_username } if join_username != &username_msgs => {
                    println!("User joined: {}", join_username);
                    log(LogLevel::Info, msgs_verbosity, &format!("User joined: {}", join_username));
                },
                ChatMessage::Leave { username: leave_username } if leave_username != &username_msgs => {
                    println!("User left: {}", leave_username);
                    log(LogLevel::Info, msgs_verbosity, &format!("User left: {}", leave_username));
                },
                ChatMessage::Text { from, content, .. } if from != &username_msgs => {
                    println!("{}: {}", from, content);
                    log(LogLevel::Info, msgs_verbosity, &format!("Message from {}: {}", from, content));
                },
                ChatMessage::StateSync { history, participants } => {
                    log(LogLevel::Debug, msgs_verbosity, &format!(
                        "Received state sync with {} messages and {} participants",
                        history.len(), participants.len()
                    ));
                    
                    println!("--- Current participants: {} ---", participants.join(", "));
                    
                    for item in history {
                        if &item.from != &username_msgs {
                            println!("[HISTORY] {}: {}", item.from, item.content);
                        }
                    }
                },
                _ => {}
            }
        } else {
            log(LogLevel::Debug, msgs_verbosity, "Failed to parse incoming message");
        }
    }).await;
    
    // Wait for Ctrl+C
    signal::ctrl_c().await?;
    
    Ok(())
}
