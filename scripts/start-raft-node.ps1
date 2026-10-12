param([Parameter(Mandatory)][int]$Id)
# Starts Raft node $Id (1-3) on port 1808$Id. Re-run the same command to restart a killed node.
$env:SAYANGCARE__QUEUE__BACKEND = "raft"
$env:SAYANGCARE__QUEUE__RAFT_PEERS = "1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083"
$env:SAYANGCARE__QUEUE__RAFT_TOKEN = "demo-secret"
$env:SAYANGCARE__QUEUE__RAFT_NODE_ID = "$Id"
$env:SAYANGCARE__SERVER__PORT = "1808$Id"
$env:SAYANGCARE__QUEUE__RAFT_DATA_DIR = "$env:TEMP\raft\n$Id"
cargo run -p sayangcare-api
