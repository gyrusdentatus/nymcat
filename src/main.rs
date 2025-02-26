// src/main.rs
mod common;

use common::{ChatMessage, HistoryItem, LogLevel, log};
use nym_sdk::mixnet::{AnonymousSenderTag, IncludedSurbs, MixnetClient, MixnetMessageSender, Recipient};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal;
use tokio::time::interval;

const STATE_SYNC_INTERVAL_SECS: u64 = 30;
const SURBS_PER_MESSAGE: u32 = 500; // Increase SURBs per message dramatically

#[derive(Default)]
struct RoomState {
    participants: HashMap<String, AnonymousSenderTag>,
    history: Vec<HistoryItem>,
}

fn get_verbosity(args: &[String]) -> LogLevel {
    for arg in args {
        match arg.as_str() {
            "-v" => return LogLevel::Info,
            "-vv" => return LogLevel::Debug,
            "-vvv" => return LogLevel::Trace,
            _ => {}
        }
    }
    LogLevel::None
}

fn broadcast_message(
    message: &ChatMessage,
    room_address: Recipient,
    _state: &Arc<Mutex<RoomState>>,
    sender: &nym_sdk::mixnet::MixnetClientSender,
    _skip_username: &str,
    verbosity: LogLevel,
) {
    let broadcast_msg = match serde_json::to_vec(&message) {
        Ok(bytes) => bytes,
        Err(e) => {
            log(LogLevel::Debug, verbosity, &format!("Failed to serialize broadcast message: {}", e));
            return;
        }
    };
    
    // Instead of using reply tags (which are single-use), send directly to the room
    // This makes the server see all messages and rebroadcast them
    log(LogLevel::Debug, verbosity, "Broadcasting message to room");
    
    let sender_clone = sender.clone();
    let broadcast_bytes = broadcast_msg.clone();
    
    tokio::spawn(async move {
        match sender_clone.send_message(room_address, &broadcast_bytes, IncludedSurbs::Amount(SURBS_PER_MESSAGE)).await {
            Ok(_) => log(LogLevel::Debug, verbosity, "Broadcast sent successfully"),
            Err(e) => log(LogLevel::Debug, verbosity, &format!("Failed to broadcast: {}", e)),
        }
    });
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} [create|join <address> <username>] [-v|-vv|-vvv]", args[0]);
        return Ok(());
    }

    let verbosity = get_verbosity(&args);

    match args[1].as_str() {
        "create" => {
            let mut client = MixnetClient::connect_new().await?;
            let room_address = *client.nym_address();
            let room_address_str = client.nym_address().to_string();
            
            println!("Room created. Address: nym://{}", room_address_str);
            log(LogLevel::Info, verbosity, &format!("Room address: {}", room_address_str));
            
            let state = Arc::new(Mutex::new(RoomState::default()));
            let state_clone = Arc::clone(&state);
            let sender = client.split_sender();
            
            // Periodic state sync
            let sync_state = Arc::clone(&state);
            let sync_sender = sender.clone();
            let sync_verbosity = verbosity;
            let sync_room_address = room_address;
            tokio::spawn(async move {
                let mut interval = interval(Duration::from_secs(STATE_SYNC_INTERVAL_SECS));
                loop {
                    interval.tick().await;
                    
                    let state_data = {
                        let state_lock = sync_state.lock().unwrap();
                        if state_lock.participants.is_empty() || state_lock.history.is_empty() {
                            log(LogLevel::Debug, sync_verbosity, "No participants or history, skipping state sync");
                            continue;
                        }
                        
                        (
                            state_lock.history.clone(),
                            state_lock.participants.keys().cloned().collect::<Vec<_>>()
                        )
                    };
                    
                    let (history, participant_names) = state_data;
                    
                    log(LogLevel::Debug, sync_verbosity, &format!("Syncing state to room with {} users", participant_names.len()));
                    
                    let sync_msg = ChatMessage::StateSync {
                        history,
                        participants: participant_names,
                    };
                    
                    let sync_bytes = match serde_json::to_vec(&sync_msg) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            log(LogLevel::Debug, sync_verbosity, &format!("Failed to serialize state sync: {}", e));
                            continue;
                        }
                    };
                    
                    // Send state sync to the room directly
                    match sync_sender.send_message(sync_room_address, &sync_bytes, IncludedSurbs::Amount(SURBS_PER_MESSAGE)).await {
                        Ok(_) => log(LogLevel::Debug, sync_verbosity, "State sync sent successfully"),
                        Err(e) => log(LogLevel::Debug, sync_verbosity, &format!("Failed to send state sync: {}", e)),
                    }
                }
            });
            
            // Main message handler
            let msg_verbosity = verbosity;
            let room_addr = room_address;
            client.on_messages(move |msg| {
                log(LogLevel::Trace, msg_verbosity, &format!("Received raw message: {} bytes", msg.message.len()));
                
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
                            
                            // Store participant info
                            {
                                let mut state_lock = state_clone.lock().unwrap();
                                state_lock.participants.insert(username.clone(), sender_tag);
                                log(LogLevel::Debug, msg_verbosity, &format!("Added {} to participants, now have {}", username, state_lock.participants.len()));
                            }
                            
                            // Send welcome message with state sync
                            let welcome_data = {
                                let state_lock = state_clone.lock().unwrap();
                                (
                                    state_lock.history.clone(),
                                    state_lock.participants.keys().cloned().collect::<Vec<_>>()
                                )
                            };
                            
                            let (history, participant_names) = welcome_data;
                            
                            let sync_msg = ChatMessage::StateSync {
                                history,
                                participants: participant_names,
                            };
                            
                            if let Ok(sync_bytes) = serde_json::to_vec(&sync_msg) {
                                let sender_clone = sender.clone();
                                tokio::spawn(async move {
                                    sender_clone.send_reply(sender_tag, &sync_bytes).await.ok();
                                });
                            }
                        } else {
                            log(LogLevel::Debug, msg_verbosity, "Received Join without sender_tag");
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
                    },
                    ChatMessage::StateSync { .. } => {
                        log(LogLevel::Debug, msg_verbosity, "Ignoring StateSync message at server");
                    }
                }
            }).await;
        },
        
        "join" => {
            if args.len() < 4 {
                eprintln!("Usage: {} join <address> <username> [-v|-vv|-vvv]", args[0]);
                return Ok(());
            }
            
            let address_str = args[2].strip_prefix("nym://").unwrap_or(&args[2]);
            let room_address = Recipient::from_str(address_str)?;
            let username = args[3].clone();
            
            let mut client = MixnetClient::connect_new().await?;
            log(LogLevel::Info, verbosity, &format!("Connected to mixnet as {}", client.nym_address()));
            
            let sender = client.split_sender();
            
            // Send join message with plenty of SURBs for replies
            let join_msg = ChatMessage::Join { username: username.clone() };
            log(LogLevel::Debug, verbosity, "Sending join message");
            sender.send_message(room_address, &serde_json::to_vec(&join_msg)?, IncludedSurbs::Amount(SURBS_PER_MESSAGE)).await?;
            
            println!("Joined chat room as {}", username);
            
            // Handle user input
            let username_input = username.clone();
            let sender_input = sender.clone();
            let input_verbosity = verbosity;
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
                            sender_input.send_message(room_address, &msg_bytes, IncludedSurbs::Amount(SURBS_PER_MESSAGE)).await.ok();
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
            tokio::spawn(async move {
                signal::ctrl_c().await.ok();
                println!("Leaving chat room...");
                log(LogLevel::Info, exit_verbosity, "Leaving chat room (Ctrl+C received)");
                
                let leave_msg = ChatMessage::Leave { username: username_exit };
                match serde_json::to_vec(&leave_msg) {
                    Ok(msg_bytes) => {
                        sender_exit.send_message(room_address, &msg_bytes, IncludedSurbs::Amount(SURBS_PER_MESSAGE)).await.ok();
                    },
                    Err(e) => {
                        log(LogLevel::Debug, exit_verbosity, &format!("Failed to serialize leave message: {}", e));
                    }
                }
                
                std::process::exit(0);
            });
            
            // Handle incoming messages
            let username_msgs = username;
            let msgs_verbosity = verbosity;
            client.on_messages(move |msg| {
                log(LogLevel::Trace, msgs_verbosity, &format!("Received raw message: {} bytes with sender_tag: {}", 
                    msg.message.len(), msg.sender_tag.is_some()));
                
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
                            log(LogLevel::Debug, msgs_verbosity, &format!("Received state sync with {} messages and {} participants", 
                                history.len(), participants.len()));
                            println!("--- Current participants: {} ---", participants.join(", "));
                            
                            for item in history {
                                if &item.from == &username_msgs {
                                    continue;
                                }
                                
                                println!("[HISTORY] {}: {}", item.from, item.content);
                                log(LogLevel::Debug, msgs_verbosity, &format!("History item: {} - {}", item.from, item.content));
                            }
                        },
                        _ => {
                            log(LogLevel::Debug, msgs_verbosity, "Ignoring message about self");
                        }
                    }
                } else {
                    log(LogLevel::Debug, msgs_verbosity, "Failed to parse incoming message");
                    if msgs_verbosity == LogLevel::Trace {
                        log(LogLevel::Trace, msgs_verbosity, &format!("Raw message: {:?}", msg.message));
                    }
                }
            }).await;
        },
        
        _ => {
            eprintln!("Unknown command: {}", args[1]);
        }
    }
    
    Ok(())
}
