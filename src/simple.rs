// src/simple.rs
use crate::common::{
    ChatMessage, HistoryItem, LogLevel, Colors, log, format_timestamp, separator
};
use nym_sdk::mixnet::{MixnetClient, MixnetMessageSender, Recipient, IncludedSurbs, AnonymousSenderTag};
use std::collections::{HashMap, VecDeque};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal;
use tokio::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH, Duration, Instant};

// Reduced from 50 to 15 to decrease network overhead while maintaining reliability
const SURBS_PER_MESSAGE: u32 = 15; 

// Maximum history items to retain (prevents unbounded memory growth)
const MAX_HISTORY_SIZE: usize = 100;

// Participant timeout in seconds (prune after this duration of inactivity)
const PARTICIPANT_TIMEOUT_SECS: u64 = 300; // 5 minutes

// Message batch size (process this many messages at once)
const BATCH_SIZE: usize = 10;

// Maximum message queue size (prevent memory exhaustion)
const MAX_QUEUE_SIZE: usize = 1000;

#[derive(Debug, Clone, Copy)]
enum MessagePriority {
    High,   // Join/Leave messages
    Medium, // State sync and system messages
    Low,    // Regular chat messages
}

#[derive(Debug)]
struct QueuedMessage {
    message: Vec<u8>,
    recipient: AnonymousSenderTag,
    priority: MessagePriority,
    timestamp: Instant,
}

#[derive(Debug)]
struct Participant {
    username: String,
    sender_tag: AnonymousSenderTag,
    last_active: SystemTime,
}

struct RoomState {
    participants: HashMap<String, Participant>,
    history: VecDeque<HistoryItem>,
    start_time: SystemTime,
    message_count: usize,
    broadcast_count: usize,
}

impl RoomState {
    fn new() -> Self {
        Self {
            participants: HashMap::new(),
            history: VecDeque::with_capacity(MAX_HISTORY_SIZE),
            start_time: SystemTime::now(),
            message_count: 0,
            broadcast_count: 0,
        }
    }

    fn add_history_item(&mut self, item: HistoryItem) {
        self.history.push_back(item);
        if self.history.len() > MAX_HISTORY_SIZE {
            self.history.pop_front();
        }
    }

    fn prune_inactive_participants(&mut self) -> Vec<String> {
        let now = SystemTime::now();
        let timeout_duration = Duration::from_secs(PARTICIPANT_TIMEOUT_SECS);
        
        let mut pruned = Vec::new();
        
        self.participants.retain(|username, participant| {
            let is_active = match now.duration_since(participant.last_active) {
                Ok(duration) => duration < timeout_duration,
                Err(_) => true, // Keep if time calculation fails
            };
            
            if !is_active {
                pruned.push(username.clone());
            }
            
            is_active
        });
        
        pruned
    }
}

pub async fn run_room_server(verbosity: LogLevel, env_file: Option<String>) -> anyhow::Result<()> {
    // Set environment if provided
    if let Some(path) = &env_file {
        std::env::set_var("NYM_ENV_FILE", path);
    }
    
    // Create a mixnet client
    let mut client = MixnetClient::connect_new().await?;
    let _room_address = *client.nym_address();
    let room_address_str = client.nym_address().to_string();
    
    // Create message queue
    let (tx, mut rx) = mpsc::channel::<QueuedMessage>(MAX_QUEUE_SIZE);
    
    // Print fancy banner
    print_welcome_banner(&room_address_str);
    
    log(LogLevel::Info, verbosity, &format!("Room address: {}", room_address_str));
    
    // Create shared state
    let state = Arc::new(Mutex::new(RoomState::new()));
    
    // Clone for pruning task
    let prune_state = Arc::clone(&state);
    let prune_verbosity = verbosity;
    let prune_tx = tx.clone();
    
    // Start periodic pruning task
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        
        loop {
            interval.tick().await;
            
            let pruned = {
                let mut state_lock = prune_state.lock().unwrap();
                state_lock.prune_inactive_participants()
            };
            
            if !pruned.is_empty() {
                log(LogLevel::Info, prune_verbosity, 
                    &format!("Pruned {} inactive participants", pruned.len()));
                
                // Send leave messages for pruned participants
                for username in pruned {
                    let leave_msg = ChatMessage::Leave { username: username.clone() };
                    
                    if let Ok(leave_bytes) = serde_json::to_vec(&leave_msg) {
                        // Broadcast to all remaining participants
                        let recipients = {
                            let state_lock = prune_state.lock().unwrap();
                            state_lock.participants.values()
                                .map(|p| p.sender_tag)
                                .collect::<Vec<_>>()
                        };
                        
                        for recipient in recipients {
                            if let Err(e) = prune_tx.send(QueuedMessage {
                                message: leave_bytes.clone(),
                                recipient,
                                priority: MessagePriority::High,
                                timestamp: Instant::now(),
                            }).await {
                                log(LogLevel::Debug, prune_verbosity, 
                                    &format!("Failed to queue leave message: {}", e));
                            }
                        }
                    }
                }
            }
        }
    });
    
    // Clone for message processor
    let sender = client.split_sender();
    let state_clone = Arc::clone(&state);
    let sender_clone = sender.clone();
    let processor_verbosity = verbosity;
    
    // Start message processing task
    tokio::spawn(async move {
        log(LogLevel::Debug, processor_verbosity, "Starting message processor");
        
        while let Some(msg) = rx.recv().await {
            // Skip if message is too old (more than 30 seconds)
            if msg.timestamp.elapsed() > Duration::from_secs(30) {
                log(LogLevel::Debug, processor_verbosity, 
                    "Skipping outdated message in queue");
                continue;
            }
            
            log(LogLevel::Trace, processor_verbosity, &format!(
                "Processing queued message of {} bytes to recipient", 
                msg.message.len()));
            
            // Send the message
            if let Err(e) = sender_clone.send_reply(msg.recipient, &msg.message).await {
                log(LogLevel::Debug, processor_verbosity, &format!(
                    "Failed to send message: {}", e));
            }
            
            // Update broadcast counter
            {
                let mut state_lock = state_clone.lock().unwrap();
                state_lock.broadcast_count += 1;
            }
            
            // Small delay to prevent flooding
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    
    // Clone for stats task
    let stats_state = Arc::clone(&state);
    let stats_verbosity = verbosity;
    
    // Start periodic statistics reporting
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 minutes
        
        loop {
            interval.tick().await;
            
            let stats = {
                let state_lock = stats_state.lock().unwrap();
                (
                    state_lock.participants.len(),
                    state_lock.message_count,
                    state_lock.broadcast_count,
                    state_lock.start_time
                )
            };
            
            let (participants, messages, broadcasts, start_time) = stats;
            
            let uptime = SystemTime::now().duration_since(start_time)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            
            let hours = uptime / 3600;
            let minutes = (uptime % 3600) / 60;
            
            log(LogLevel::Info, stats_verbosity, &format!(
                "Stats: {} participants, {} messages, {} broadcasts, uptime: {}h {}m",
                participants, messages, broadcasts, hours, minutes
            ));
        }
    });
    
    // Clone for message handler
    let state_clone = Arc::clone(&state);
    let msg_tx = tx.clone();
    let msg_verbosity = verbosity;
    
    // Handle messages
    client.on_messages(move |msg| {
        log(LogLevel::Trace, msg_verbosity, &format!(
            "Received raw message: {} bytes", msg.message.len()));
        
        // Try to parse the message
        let message: ChatMessage = match serde_json::from_slice(&msg.message) {
            Ok(m) => m,
            Err(e) => {
                log(LogLevel::Debug, msg_verbosity, &format!("Failed to parse message: {}", e));
                return;
            }
        };
        
        let sender_tag = match msg.sender_tag {
            Some(tag) => tag,
            None => {
                log(LogLevel::Debug, msg_verbosity, "Message has no sender tag, skipping");
                return;
            }
        };
        
        match &message {
            ChatMessage::Join { username } => {
                println!("{}User joined:{} {}", Colors::GREEN, Colors::RESET, username);
                log(LogLevel::Info, msg_verbosity, &format!(
                    "User joined: {} with sender tag", username));
                
                // Store participant with last active time
                {
                    let mut state_lock = state_clone.lock().unwrap();
                    state_lock.participants.insert(username.clone(), Participant {
                        username: username.clone(),
                        sender_tag,
                        last_active: SystemTime::now(),
                    });
                    
                    state_lock.message_count += 1;
                }
                
                // Send state sync to new user
                let state_data = {
                    let state_lock = state_clone.lock().unwrap();
                    (
                        Vec::from(state_lock.history.clone()),
                        state_lock.participants.values().map(|p| p.username.clone()).collect::<Vec<_>>()
                    )
                };
                
                let (history, participants) = state_data;
                let sync_msg = ChatMessage::StateSync {
                    history,
                    participants,
                };
                
                if let Ok(sync_bytes) = serde_json::to_vec(&sync_msg) {
                    let tx_clone = msg_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = tx_clone.send(QueuedMessage {
                            message: sync_bytes,
                            recipient: sender_tag,
                            priority: MessagePriority::Medium,
                            timestamp: Instant::now(),
                        }).await {
                            eprintln!("Failed to queue sync: {}", e);
                        }
                    });
                }
                
                // Broadcast join to others
                if let Ok(join_bytes) = serde_json::to_vec(&message) {
                    broadcast_to_participants(
                        &join_bytes, 
                        &state_clone, 
                        &msg_tx, 
                        username, 
                        MessagePriority::High,
                        msg_verbosity
                    );
                }
            },
            ChatMessage::Leave { username } => {
                println!("{}User left:{} {}", Colors::YELLOW, Colors::RESET, username);
                log(LogLevel::Info, msg_verbosity, &format!("User left: {}", username));
                
                // Remove participant
                {
                    let mut state_lock = state_clone.lock().unwrap();
                    state_lock.participants.remove(username);
                    state_lock.message_count += 1;
                }
                
                // Broadcast leave to others
                if let Ok(leave_bytes) = serde_json::to_vec(&message) {
                    broadcast_to_participants(
                        &leave_bytes, 
                        &state_clone, 
                        &msg_tx, 
                        username, 
                        MessagePriority::High,
                        msg_verbosity
                    );
                }
            },
            ChatMessage::Text { from, content, timestamp } => {
                println!("{}: {}", from, content);
                log(LogLevel::Info, msg_verbosity, &format!("Message from {}: {}", from, content));
                
                // Update last active time
                {
                    let mut state_lock = state_clone.lock().unwrap();
                    if let Some(participant) = state_lock.participants.get_mut(from) {
                        participant.last_active = SystemTime::now();
                    }
                }
                
                // Store in history
                {
                    let mut state_lock = state_clone.lock().unwrap();
                    let history_item = HistoryItem {
                        from: from.clone(),
                        content: content.clone(),
                        timestamp: *timestamp,
                    };
                    state_lock.add_history_item(history_item);
                    state_lock.message_count += 1;
                }
                
                // Broadcast message to others
                if let Ok(text_bytes) = serde_json::to_vec(&message) {
                    broadcast_to_participants(
                        &text_bytes, 
                        &state_clone, 
                        &msg_tx, 
                        from, 
                        MessagePriority::Low,
                        msg_verbosity
                    );
                }
            },
            ChatMessage::StateSync { .. } => {
                log(LogLevel::Debug, msg_verbosity, "Ignoring StateSync message at server");
            }
        }
    }).await;
    
    // Print server status
    println!("\n{}", separator(Some("Server Status"), 80));
    println!("Room is running and waiting for connections.");
    println!("Press Ctrl+C to shutdown the server.");
    println!("{}\n", separator(None, 80));
    
    // Wait for Ctrl+C
    signal::ctrl_c().await?;
    println!("{}Shutting down room server...{}", Colors::YELLOW, Colors::RESET);
    
    // Print final stats
    {
        let state_lock = state.lock().unwrap();
        let uptime = SystemTime::now().duration_since(state_lock.start_time)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        
        let hours = uptime / 3600;
        let minutes = (uptime % 3600) / 60;
        
        println!("\n{}", separator(Some("Final Statistics"), 80));
        println!("Uptime: {}h {}m", hours, minutes);
        println!("Total participants: {}", state_lock.participants.len());
        println!("Total messages processed: {}", state_lock.message_count);
        println!("Total broadcasts sent: {}", state_lock.broadcast_count);
        println!("{}\n", separator(None, 80));
    }
    
    Ok(())
}

fn broadcast_to_participants(
    message: &[u8],
    state: &Arc<Mutex<RoomState>>,
    tx: &mpsc::Sender<QueuedMessage>,
    skip_username: &str,
    priority: MessagePriority,
    verbosity: LogLevel,
) {
    // Get participants to broadcast to
    let recipients = {
        let state_lock = state.lock().unwrap();
        state_lock.participants.values()
            .filter(|p| p.username != skip_username)
            .map(|p| p.sender_tag)
            .collect::<Vec<_>>()
    };
    
    if recipients.is_empty() {
        return;
    }
    
    log(LogLevel::Debug, verbosity, &format!(
        "Broadcasting message to {} recipients", recipients.len()
    ));
    
    // Queue the broadcasts
    for recipient in recipients {
        let tx_clone = tx.clone();
        let message_clone = message.to_vec();
        
        tokio::spawn(async move {
            if let Err(e) = tx_clone.send(QueuedMessage {
                message: message_clone,
                recipient,
                priority,
                timestamp: Instant::now(),
            }).await {
                eprintln!("Failed to queue broadcast: {}", e);
            }
        });
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
    sender.send_message(
        room_address, 
        &serde_json::to_vec(&join_msg)?, 
        IncludedSurbs::Amount(SURBS_PER_MESSAGE)
    ).await?;
    
    println!("{}Joined chat room as {}{}", Colors::GREEN, username, Colors::RESET);
    
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
                        eprintln!("{}Failed to send message: {}{}", Colors::RED, e, Colors::RESET);
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
        println!("{}Leaving chat room...{}", Colors::YELLOW, Colors::RESET);
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
                    println!("{}User joined:{} {}", Colors::GREEN, Colors::RESET, join_username);
                    log(LogLevel::Info, msgs_verbosity, &format!("User joined: {}", join_username));
                },
                ChatMessage::Leave { username: leave_username } if leave_username != &username_msgs => {
                    println!("{}User left:{} {}", Colors::YELLOW, Colors::RESET, leave_username);
                    log(LogLevel::Info, msgs_verbosity, &format!("User left: {}", leave_username));
                },
                ChatMessage::Text { from, content, .. } if from != &username_msgs => {
                    let name_color = get_username_color(from);
                    println!("{}{}{}: {}", name_color, from, Colors::RESET, content);
                    log(LogLevel::Info, msgs_verbosity, &format!("Message from {}: {}", from, content));
                },
                ChatMessage::StateSync { history, participants } => {
                    log(LogLevel::Debug, msgs_verbosity, &format!(
                        "Received state sync with {} messages and {} participants",
                        history.len(), participants.len()
                    ));
                    
                    // Print participant list
                    println!("\n{}", separator(Some(&format!("Current Participants ({})", participants.len())), 80));
                    
                    for participant in participants {
                        let color = if *participant == username_msgs {
                            Colors::BRIGHT_BLUE
                        } else {
                            get_username_color(&participant)
                        };
                        println!("- {}{}{}", color, participant, Colors::RESET);
                    }
                    
                    println!("{}", separator(None, 80));
                    
                    // Print history
                    if !history.is_empty() {
                        println!("{}", separator(Some("Message History"), 80));
                        
                        for item in history {
                            if &item.from != &username_msgs {
                                // Format timestamp
                                let time = UNIX_EPOCH + Duration::from_secs(item.timestamp);
                                let time_str = format_timestamp(time);
                                
                                // Get username color
                                let name_color = get_username_color(&item.from);
                                
                                println!("{}{}{} {}{}{}: {}",
                                    Colors::DIM, time_str, Colors::RESET,
                                    name_color, item.from, Colors::RESET,
                                    item.content
                                );
                            }
                        }
                        
                        println!("{}", separator(None, 80));
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

// Print welcome banner for the server
fn print_welcome_banner(address: &str) {
    println!("{}", Colors::BRIGHT_CYAN);
    println!(r"

,  ,  ,  ,, ,  _,_  ___,
|\ |  \_/|\/| / '|\' |  
|'\| , /`| `|'\_ |-\ |  
'  `(_/  '  `   `'  `' 
    
    chat with your catz and get mixxed up in some serious sh1t among
    the other sphinx packets with SURBs.
    No messages stored, no rooms persist, it all turns to dust.");
    println!("{}", Colors::RESET);
    
    println!("{}{}{} Room Server {}{}{}\n",
        Colors::DIM,
        "•",
        Colors::RESET,
        Colors::DIM,
        "•",
        Colors::RESET
    );
    
    println!("{}Room Address:{} nym://{}\n", 
        Colors::BRIGHT_YELLOW, 
        Colors::RESET,
        address
    );
}

// Get a consistent color for a username (copied from common)
fn get_username_color(username: &str) -> &'static str {
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
