#!/bin/bash
# Mock CLI for persistent process integration tests.
# Handles initialize once, then loops processing user messages until stdin closes.

# Read initialize
read -r line

# Emit init
echo '{"type":"system","subtype":"init","model":"mock-persistent","session_id":"mock-persistent-001"}'

# Process messages in a loop
while read -r line; do
    if echo "$line" | grep -q '"type":"user"'; then
        echo '{"type":"assistant","session_id":"mock-persistent-001","message":{"content":[{"type":"text","text":"Persistent response"}]}}'
        echo '{"type":"result","subtype":"success","session_id":"mock-persistent-001","is_error":false,"result":"Done"}'
    fi
done
