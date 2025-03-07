# nymcat

A privacy-focused chat application that operates over the Nym mixnet.

## Overview

`nymcat` is a terminal-based chat application that uses the Nym mixnet to provide metadata-protected communication. All messages are routed through multiple mix nodes, making it extremely difficult for adversaries to determine who is communicating with whom.

Key features:
- **Hidden service architecture**: Chat rooms exist as services on the mixnet with no exposed IP address
- **Zero-knowledge messaging**: No central server knows who is communicating with whom
- **Works behind NAT**: No port forwarding or public IP required
- **Terminal UI**: Simple interface accessible from any terminal
![image](https://github.com/user-attachments/assets/6914b86c-5536-43d7-86f8-b530f9a6bace)

<img width="1369" alt="image" src="https://github.com/user-attachments/assets/97db20ae-4411-4283-9856-062ec8e447f8" />

## Installation

```bash
# Clone the repository
git clone https://github.com/gyrusdentatus/nymcat.git
cd nymcat

# Build the application
cargo build --release

# Run the application
./target/release/nymcat
```

## Usage

### Creating a chat room

```bash
nymcat create -vvv
```

This will output a Nym address for your chat room in the format:
```
Room created. Address: nym://HQv8fYN7NaQJmJfMpemF7KCw86XPVP7jgPED1SkjC1Hn.HyWwPsvupewvcdeJ8c2Ppo9no5nrvhbezBTU1jQa8cmc@7ntzmDZRvG4a1pnDBU4Bg1RiAmLwmqXV5sZGNw68Ce14
```

### Joining a chat room

```bash
nymcat join <room-address> <username> -vvv
```

For example:
```bash
nymcat join nym://HQv8fYN7NaQJmJfMpemF7KCw86XPVP7jgPED1SkjC1Hn.HyWwPsvupewvcdeJ8c2Ppo9no5nrvhbezBTU1jQa8cmc@7ntzmDZRvG4a1pnDBU4Bg1RiAmLwmqXV5sZGNw68Ce14 Alice -vvv
```

### Chatting

Once joined, simply type messages and press Enter to send. Messages from other participants will appear in your terminal.

### Leaving a chat room

Press Ctrl+C to leave gracefully.

## How It Works

`nymcat` leverages Nym's Sphinx packet format and mixnet architecture to provide:

1. **Metadata protection**: Hides who is communicating with whom
2. **Timing obfuscation**: Messages are delayed randomly to prevent timing analysis
3. **Cover traffic**: Additional dummy traffic masks usage patterns
4. **Location hiding**: Chat rooms exist as "hidden services" with no exposed IP

Each message traverses multiple mix nodes, with each node:
- Decrypting one layer of the message
- Mixing it with other traffic
- Adding random delays
- Forwarding to the next node

## Privacy Considerations

- `nymcat` protects metadata but does not yet implement end-to-end encryption for message contents
- Username selection should avoid identifying information
- Extended chat sessions can potentially leak information through message patterns

## Development

Contributions are welcome! `nymcat` is built with:

- Rust
- Nym SDK (mixnet client)
- Tokio (async runtime)

## License

MIT License
