// src/main.rs
mod common;

use common::ChatMessage;
use nym_sdk::mixnet::{IncludedSurbs, MixnetClient, MixnetMessageSender, Recipient};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} [create|join <address> <username>]", args[0]);
        return Ok(());
    }

    match args[1].as_str() {
        "create" => {
            let mut client = MixnetClient::connect_new().await?;
            let room_address = client.nym_address().to_string();
            
            println!("Room created. Address: nym://{}", room_address);
            
            let participants = Arc::new(Mutex::new(HashMap::new()));
            let participants_clone = Arc::clone(&participants);
            let sender = client.split_sender();
            
            client.on_messages(move |msg| {
                let message: ChatMessage = match serde_json::from_slice(&msg.message) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Failed to parse message: {}", e);
                        return;
                    }
                };
                
                match &message {
                    ChatMessage::Join { username } => {
                        if let Some(sender_tag) = msg.sender_tag {
                            participants_clone.lock().unwrap().insert(username.clone(), sender_tag);
                            println!("User joined: {}", username);
                        }
                    },
                    ChatMessage::Leave { username } => {
                        participants_clone.lock().unwrap().remove(username);
                        println!("User left: {}", username);
                    },
                    ChatMessage::Text { from, ref content } => {
                        println!("{}: {}", from, content);
                        
                        // Broadcast to all participants
                        let broadcast_msg = serde_json::to_vec(&message).unwrap_or_default();
                        let participant_copy = participants_clone.lock().unwrap().clone();
                        
                        for (_, sender_tag) in participant_copy {
                            let sender_clone = sender.clone();
                            let broadcast_bytes = broadcast_msg.clone();
                            
                            tokio::spawn(async move {
                                sender_clone.send_reply(sender_tag, &broadcast_bytes).await.ok();
                            });
                        }
                    }
                }
            }).await;
        },
        
        "join" => {
            if args.len() < 4 {
                eprintln!("Usage: {} join <address> <username>", args[0]);
                return Ok(());
            }
            
            let address_str = args[2].strip_prefix("nym://").unwrap_or(&args[2]);
            let room_address = Recipient::from_str(address_str)?;
            let username = args[3].clone();
            
            let mut client = MixnetClient::connect_new().await?;
            let sender = client.split_sender();
            
            // Send join message with SURBs for replies
            let join_msg = ChatMessage::Join { username: username.clone() };
            sender.send_message(room_address, &serde_json::to_vec(&join_msg)?, IncludedSurbs::Amount(10)).await?;
            
            println!("Joined chat room as {}", username);
            
            // Handle user input
            let username_input = username.clone();
            let sender_input = sender.clone();
            tokio::spawn(async move {
                let stdin = BufReader::new(tokio::io::stdin());
                let mut lines = stdin.lines();
                
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() { continue; }
                    
                    let text_msg = ChatMessage::Text {
                        from: username_input.clone(),
                        content: line.trim().to_string(),
                    };
                    
                    sender_input.send_message(room_address, &serde_json::to_vec(&text_msg).unwrap(), IncludedSurbs::Amount(10)).await.ok();
                }
            });
            
            // Handle Ctrl+C for clean exit
            let username_exit = username.clone();
            let sender_exit = sender.clone();
            tokio::spawn(async move {
                signal::ctrl_c().await.ok();
                println!("Leaving chat room...");
                
                let leave_msg = ChatMessage::Leave { username: username_exit };
                sender_exit.send_message(room_address, &serde_json::to_vec(&leave_msg).unwrap(), IncludedSurbs::Amount(10)).await.ok();
                
                std::process::exit(0);
            });
            
            // Handle incoming messages
            let username_msgs = username;
            client.on_messages(move |msg| {
                if let Ok(message) = serde_json::from_slice::<ChatMessage>(&msg.message) {
                    match message {
                        ChatMessage::Join { username: join_username } if join_username != username_msgs => {
                            println!("User joined: {}", join_username);
                        },
                        ChatMessage::Leave { username: leave_username } if leave_username != username_msgs => {
                            println!("User left: {}", leave_username);
                        },
                        ChatMessage::Text { from, content } if from != username_msgs => {
                            println!("{}: {}", from, content);
                        },
                        _ => {}
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
