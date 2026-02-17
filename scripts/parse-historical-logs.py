#!/usr/bin/env python3
"""
Parse historical OpenClaw gateway.log and extract sessions/events.
Posts to Watchtower API to populate historical data.

Usage: ./parse-historical-logs.py [--hours N] [--dry-run]
"""

import json
import sys
import requests
from datetime import datetime, timedelta, timezone
from collections import defaultdict
from pathlib import Path

LOG_PATH = Path.home() / ".openclaw/logs/gateway.log"
API_BASE = "http://localhost:3002/api"

# Session tracking
sessions = {}  # session_id -> session_data
events_to_post = []

def parse_subsystem(name):
    """Extract subsystem from log name field."""
    if not name or not name.startswith('{'):
        return name
    try:
        parsed = json.loads(name)
        return parsed.get('subsystem') or parsed.get('module')
    except:
        return name

def extract_field(message, field):
    """Extract field=value from message string."""
    if field not in message:
        return None
    try:
        idx = message.index(field)
        rest = message[idx + len(field):]
        end = len(rest)
        for i, c in enumerate(rest):
            if c in ' \t,)':
                end = i
                break
        return rest[:end].strip('"')
    except:
        return None

def determine_session_type(session_key):
    """Determine session type from key."""
    if ':cron:' in session_key:
        return 'cron'
    elif ':subagent:' in session_key:
        return 'subagent'
    elif ':main:' in session_key or 'agent:main:' in session_key:
        return 'main'
    return 'unknown'

def categorize_event(message, subsystem, level, timestamp):
    """Categorize a log message into an event."""
    msg_lower = message.lower()
    sub_lower = (subsystem or '').lower()
    
    # Skip noisy messages
    if any(x in msg_lower for x in [
        'lane enqueue', 'lane dequeue', 'run cleared', 
        'lane task done', 'cron: timer armed'
    ]):
        return None
    
    # Errors
    if level == 'ERROR':
        return {
            'event_type': 'alert',
            'summary': message[:100] + '...' if len(message) > 100 else message,
            'detail': message,
            'timestamp': timestamp,
        }
    
    # Warnings
    if level == 'WARN':
        return {
            'event_type': 'warning',
            'summary': message[:100] + '...' if len(message) > 100 else message,
            'detail': message,
            'timestamp': timestamp,
        }
    
    # Agent runs
    if 'embedded run start:' in msg_lower:
        model = extract_field(message, 'model=') or 'unknown'
        return {
            'event_type': 'agent',
            'summary': f'🚀 Agent run started ({model})',
            'detail': message,
            'session_id': extract_field(message, 'sessionId='),
            'timestamp': timestamp,
        }
    
    if 'embedded run done:' in msg_lower:
        duration = extract_field(message, 'durationMs=') or '?'
        return {
            'event_type': 'agent',
            'summary': f'✅ Agent run completed ({duration}ms)',
            'detail': message,
            'session_id': extract_field(message, 'sessionId='),
            'timestamp': timestamp,
        }
    
    # Session state
    if 'session state:' in msg_lower:
        new_state = extract_field(message, 'new=') or '?'
        reason = extract_field(message, 'reason=') or ''
        session_key = extract_field(message, 'sessionKey=') or ''
        
        emoji = '🕐' if ':cron:' in session_key else ('🤖' if ':subagent:' in session_key else '👤')
        state_desc = 'working' if new_state == 'processing' else new_state
        
        return {
            'event_type': 'agent',
            'summary': f'{emoji} Session {state_desc} ({reason})',
            'detail': f'Key: {session_key}',
            'session_id': extract_field(message, 'sessionId='),
            'timestamp': timestamp,
        }
    
    # Tool usage
    if 'embedded run tool start:' in msg_lower:
        tool = extract_field(message, 'tool=') or 'unknown'
        tool_type = {
            'exec': 'shell', 'read': 'file', 'write': 'file', 'edit': 'file',
            'web_search': 'api', 'web_fetch': 'api', 'message': 'message',
        }.get(tool.lower(), 'info')
        return {
            'event_type': tool_type,
            'summary': f'🔧 Tool: {tool}',
            'detail': message,
            'session_id': extract_field(message, 'runId='),
            'timestamp': timestamp,
        }
    
    # Shell commands
    if 'exec' in sub_lower or 'elevated command' in msg_lower:
        cmd = message
        if 'elevated command ' in message:
            cmd = message.split('elevated command ', 1)[-1]
        return {
            'event_type': 'shell',
            'summary': f'🖥️ {cmd[:80]}...' if len(cmd) > 80 else f'🖥️ {cmd}',
            'detail': cmd,
            'timestamp': timestamp,
        }
    
    # Cron
    if 'cron' in sub_lower:
        return {
            'event_type': 'cron',
            'summary': message[:80] + '...' if len(message) > 80 else message,
            'detail': message,
            'timestamp': timestamp,
        }
    
    # Skip DEBUG
    if level == 'DEBUG':
        return None
    
    return None  # Skip unmatched INFO messages

def process_session(message, timestamp):
    """Track session lifecycle."""
    if 'session state:' not in message:
        return None
    
    session_id = extract_field(message, 'sessionId=')
    session_key = extract_field(message, 'sessionKey=')
    new_state = extract_field(message, 'new=')
    
    if not session_id or not session_key:
        return None
    
    if new_state == 'processing':
        # Session started
        sessions[session_id] = {
            'session_key': session_key,
            'session_type': determine_session_type(session_key),
            'started_at': timestamp,
        }
    elif new_state == 'idle' and session_id in sessions:
        # Session ended
        sess = sessions.pop(session_id)
        sess['ended_at'] = timestamp
        return sess
    
    return None

def post_event(event, dry_run=False):
    """Post event to API."""
    if dry_run:
        print(f"  [DRY RUN] Would post event: {event['event_type']} - {event['summary'][:50]}")
        return True
    
    try:
        resp = requests.post(f"{API_BASE}/events", json={
            'event_type': event['event_type'],
            'summary': event['summary'],
            'detail': event.get('detail'),
            'session_id': event.get('session_id'),
        }, timeout=5)
        return resp.status_code in (200, 201)
    except Exception as e:
        print(f"  Error posting event: {e}")
        return False

def post_session(session, dry_run=False):
    """Post session to API."""
    if dry_run:
        print(f"  [DRY RUN] Would post session: {session['session_key']}")
        return True
    
    try:
        resp = requests.post(f"{API_BASE}/sessions", json={
            'session_key': session['session_key'],
            'session_type': session['session_type'],
            'title': f"Historical session ({session['session_type']})",
        }, timeout=5)
        return resp.status_code in (200, 201)
    except Exception as e:
        print(f"  Error posting session: {e}")
        return False

def main():
    import argparse
    parser = argparse.ArgumentParser(description='Parse historical OpenClaw logs')
    parser.add_argument('--hours', type=int, default=24, help='Hours of history to parse (default: 24)')
    parser.add_argument('--dry-run', action='store_true', help='Print what would be posted without posting')
    args = parser.parse_args()
    
    if not LOG_PATH.exists():
        print(f"Log file not found: {LOG_PATH}")
        sys.exit(1)
    
    cutoff = datetime.now(timezone.utc) - timedelta(hours=args.hours)
    print(f"Parsing logs from the last {args.hours} hours (since {cutoff.isoformat()})")
    
    events_count = 0
    sessions_count = 0
    lines_processed = 0
    
    with open(LOG_PATH, 'r') as f:
        for line in f:
            lines_processed += 1
            if lines_processed % 10000 == 0:
                print(f"  Processed {lines_processed} lines...")
            
            line = line.strip()
            if not line:
                continue
            
            try:
                data = json.loads(line)
            except:
                continue
            
            # Get timestamp
            time_str = data.get('time')
            if not time_str:
                continue
            
            try:
                ts = datetime.fromisoformat(time_str.replace('Z', '+00:00'))
            except:
                continue
            
            # Skip if before cutoff
            if ts < cutoff:
                continue
            
            # Extract fields
            meta = data.get('_meta', {})
            level = meta.get('logLevelName', 'INFO')
            name = meta.get('name', '')
            subsystem = parse_subsystem(name)
            
            # Get message
            msg = data.get('1')
            if isinstance(msg, dict):
                msg = json.dumps(msg)
            elif msg is None:
                msg = data.get('0', '')
            
            # Process session state
            session_data = process_session(str(msg), time_str)
            if session_data:
                if post_session(session_data, args.dry_run):
                    sessions_count += 1
            
            # Categorize and post event
            event = categorize_event(str(msg), subsystem, level, time_str)
            if event:
                if post_event(event, args.dry_run):
                    events_count += 1
    
    print(f"\nDone! Processed {lines_processed} lines")
    print(f"Posted {events_count} events and {sessions_count} sessions")

if __name__ == '__main__':
    main()
