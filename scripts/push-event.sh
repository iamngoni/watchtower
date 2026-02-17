#!/bin/bash
# Push an event to Watchtower
# Usage: push-event.sh <type> <summary> [detail] [session_id] [task_id]
#
# Examples:
#   push-event.sh deployment "Deployed Kompressor v1.2.3"
#   push-event.sh error "Backup failed" "Timeout after 120s" "session-123"
#   push-event.sh task_completed "Fixed bug #42" "" "" 42

TYPE="${1:-info}"
SUMMARY="${2:-Event}"
DETAIL="${3:-}"
SESSION_ID="${4:-}"
TASK_ID="${5:-null}"

# Build JSON payload
if [ -n "$SESSION_ID" ]; then
    SESSION_JSON="\"session_id\": \"$SESSION_ID\","
else
    SESSION_JSON=""
fi

if [ "$TASK_ID" != "null" ] && [ -n "$TASK_ID" ]; then
    TASK_JSON="\"task_id\": $TASK_ID"
else
    TASK_JSON="\"task_id\": null"
fi

curl -s -X POST http://localhost:3002/api/events \
  -H "Content-Type: application/json" \
  -d "{
    \"event_type\": \"$TYPE\",
    \"summary\": \"$SUMMARY\",
    \"detail\": \"$DETAIL\",
    $SESSION_JSON
    $TASK_JSON
  }"
