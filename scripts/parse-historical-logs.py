#!/usr/bin/env python3
"""
Parse historical OpenClaw gateway.log and extract sessions/events.
Posts to Watchtower API to populate historical data.

Usage: ./parse-historical-logs.py [--hours N] [--days N] [--dry-run]
"""

import json
import sys
import re
import requests
from datetime import datetime, timedelta, timezone
from collections import defaultdict
from pathlib import Path

LOG_PATH = Path.home() / ".openclaw/logs/gateway.log"
API_BASE = "http://localhost:3002/api"

# Model pricing per 1M tokens
MODEL_PRICING = {
    'claude-3-opus': {'input': 15.0, 'output': 75.0},
    'claude-3-5-sonnet': {'input': 3.0, 'output': 15.0},
    'claude-3-sonnet': {'input': 3.0, 'output': 15.0},
    'claude-3-haiku': {'input': 0.25, 'output': 1.25},
    'claude-opus-4-5': {'input': 15.0, 'output': 75.0},
    'claude-opus-4-6': {'input': 15.0, 'output': 75.0},
    'claude-sonnet-4': {'input': 3.0, 'output': 15.0},
    'gpt-4': {'input': 30.0, 'output': 60.0},
    'gpt-4o': {'input': 2.5, 'output': 10.0},
    'gpt-4o-mini': {'input': 0.15, 'output': 0.6},
}

# Session tracking
sessions = {}  # session_key -> session_data
completed_sessions = []

def get_model_cost(model, input_tokens, output_tokens):
    """Calculate cost for model usage."""
    model_lower = model.lower() if model else ''
    
    for key, pricing in MODEL_PRICING.items():
        if key in model_lower:
            input_cost = (input_tokens / 1_000_000) * pricing['input']
            output_cost = (output_tokens / 1_000_000) * pricing['output']
            return input_cost + output_cost
    
    # Default to opus pricing if unknown
    return (input_tokens / 1_000_000) * 15.0 + (output_tokens / 1_000_000) * 75.0

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
    if not message or field not in message:
        return None
    try:
        # Handle both field=value and field="value" formats
        pattern = rf'{re.escape(field)}=(?:"([^"]+)"|(\S+))'
        match = re.search(pattern, message)
        if match:
            return match.group(1) or match.group(2)
    except:
        pass
    return None

def determine_session_type(session_key):
    """Determine session type from key."""
    if not session_key:
        return 'conversation'
    if ':cron:' in session_key:
        return 'cron'
    elif ':subagent:' in session_key or ':sub:' in session_key:
        return 'sub_agent'
    elif ':main:' in session_key:
        return 'conversation'
    return 'conversation'

def parse_token_info(message):
    """Extract token usage from various log formats."""
    tokens = {'input': 0, 'output': 0, 'cache_read': 0, 'cache_write': 0}
    
    # Try different patterns
    patterns = [
        (r'inputTokens?[=:]\s*(\d+)', 'input'),
        (r'outputTokens?[=:]\s*(\d+)', 'output'),
        (r'input_tokens[=:]\s*(\d+)', 'input'),
        (r'output_tokens[=:]\s*(\d+)', 'output'),
        (r'cacheReadTokens?[=:]\s*(\d+)', 'cache_read'),
        (r'cacheWriteTokens?[=:]\s*(\d+)', 'cache_write'),
    ]
    
    for pattern, key in patterns:
        match = re.search(pattern, message, re.IGNORECASE)
        if match:
            tokens[key] = int(match.group(1))
    
    return tokens

def categorize_event(message, subsystem, level, timestamp):
    """Categorize a log message into an event."""
    msg_lower = (message or '').lower()
    sub_lower = (subsystem or '').lower()
    
    # Skip noisy messages
    skip_patterns = [
        'lane enqueue', 'lane dequeue', 'run cleared', 
        'lane task done', 'cron: timer armed', 'heartbeat',
        'pre-prompt:', 'post-prompt:', 'context-diag'
    ]
    if any(x in msg_lower for x in skip_patterns):
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
        session_id = extract_field(message, 'sessionId=')
        return {
            'event_type': 'agent',
            'summary': f'🚀 Agent run started ({model})',
            'detail': message,
            'session_id': session_id,
            'timestamp': timestamp,
            'metadata': {'model': model, 'type': 'run_start'}
        }
    
    if 'embedded run done:' in msg_lower or 'embedded run complete' in msg_lower:
        duration = extract_field(message, 'durationMs=') or '?'
        return {
            'event_type': 'agent',
            'summary': f'✅ Agent run completed ({duration}ms)',
            'detail': message,
            'session_id': extract_field(message, 'sessionId='),
            'timestamp': timestamp,
            'metadata': {'duration_ms': duration, 'type': 'run_complete'}
        }
    
    # Telegram messages sent
    if 'telegram' in sub_lower and ('send' in msg_lower or 'message' in msg_lower):
        return {
            'event_type': 'message',
            'summary': f'💬 Telegram message',
            'detail': message[:200],
            'timestamp': timestamp,
        }
    
    # Tool usage
    if 'embedded run tool start:' in msg_lower or 'tool start:' in msg_lower:
        tool = extract_field(message, 'tool=') or 'unknown'
        tool_type = {
            'exec': 'shell', 'read': 'file', 'write': 'file', 'edit': 'file',
            'web_search': 'api', 'web_fetch': 'api', 'message': 'message',
            'browser': 'api', 'image': 'api',
        }.get(tool.lower(), 'info')
        return {
            'event_type': tool_type,
            'summary': f'🔧 Tool: {tool}',
            'detail': message[:200],
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
            'detail': cmd[:500],
            'timestamp': timestamp,
        }
    
    # Skip DEBUG
    if level == 'DEBUG':
        return None
    
    return None

def process_session(message, timestamp, model=None):
    """Track session lifecycle from log messages."""
    if not message:
        return None
    
    # Handle session state changes
    if 'session state:' in message:
        session_id = extract_field(message, 'sessionId=')
        session_key = extract_field(message, 'sessionKey=')
        new_state = extract_field(message, 'new=')
        
        if not session_key:
            return None
        
        if new_state == 'processing':
            # Session started
            if session_key not in sessions:
                sessions[session_key] = {
                    'session_key': session_key,
                    'session_id': session_id,
                    'session_type': determine_session_type(session_key),
                    'started_at': timestamp,
                    'model': model,
                    'input_tokens': 0,
                    'output_tokens': 0,
                    'runs': 0,
                }
        elif new_state == 'idle' and session_key in sessions:
            # Session idle - accumulate info
            sess = sessions[session_key]
            sess['runs'] = sess.get('runs', 0) + 1
        
        return None
    
    # Handle embedded run start (captures model)
    if 'embedded run start:' in message:
        session_id = extract_field(message, 'sessionId=')
        run_model = extract_field(message, 'model=')
        
        # Find matching session by ID
        for key, sess in sessions.items():
            if sess.get('session_id') == session_id:
                if run_model:
                    sess['model'] = run_model
                break
        
        return None
    
    # Handle token usage in various formats
    if any(x in message.lower() for x in ['token', 'usage', 'completion']):
        tokens = parse_token_info(message)
        
        # Try to find the session to update
        session_id = extract_field(message, 'sessionId=')
        for key, sess in sessions.items():
            if sess.get('session_id') == session_id or (not session_id and sess.get('runs', 0) > 0):
                sess['input_tokens'] = sess.get('input_tokens', 0) + tokens['input']
                sess['output_tokens'] = sess.get('output_tokens', 0) + tokens['output']
                break
    
    return None

def finalize_sessions():
    """Convert active sessions to completed ones for posting."""
    now = datetime.now(timezone.utc).isoformat()
    for key, sess in sessions.items():
        if sess.get('runs', 0) > 0 or sess.get('input_tokens', 0) > 0:
            completed_sessions.append({
                'session_key': sess['session_key'],
                'session_type': sess['session_type'],
                'title': generate_session_title(sess),
                'model': sess.get('model'),
                'input_tokens': sess.get('input_tokens', 0),
                'output_tokens': sess.get('output_tokens', 0),
                'cost_usd': get_model_cost(
                    sess.get('model', ''),
                    sess.get('input_tokens', 0),
                    sess.get('output_tokens', 0)
                ),
                'started_at': sess.get('started_at'),
            })

def generate_session_title(sess):
    """Generate a descriptive title for the session."""
    stype = sess.get('session_type', 'conversation')
    model = sess.get('model', 'unknown')
    
    # Clean up model name
    if '/' in model:
        model = model.split('/')[-1]
    
    type_emojis = {
        'cron': '⏰',
        'sub_agent': '🤖',
        'conversation': '💬',
    }
    emoji = type_emojis.get(stype, '💬')
    
    return f"{emoji} {stype.replace('_', ' ').title()} ({model})"

def post_event(event, dry_run=False):
    """Post event to API."""
    if dry_run:
        print(f"  [DRY RUN] Event: {event['event_type']} - {event['summary'][:50]}")
        return True
    
    try:
        payload = {
            'event_type': event['event_type'],
            'summary': event['summary'],
            'detail': event.get('detail'),
            'session_id': event.get('session_id'),
        }
        if event.get('metadata'):
            payload['metadata'] = event['metadata']
        
        resp = requests.post(f"{API_BASE}/events", json=payload, timeout=5)
        return resp.status_code in (200, 201)
    except Exception as e:
        print(f"  Error posting event: {e}")
        return False

def post_session(session, dry_run=False):
    """Post session to API."""
    if dry_run:
        tokens = session.get('input_tokens', 0) + session.get('output_tokens', 0)
        cost = session.get('cost_usd', 0)
        print(f"  [DRY RUN] Session: {session['session_key'][:50]} ({tokens} tokens, ${cost:.4f})")
        return True
    
    try:
        resp = requests.post(f"{API_BASE}/sessions", json={
            'session_key': session['session_key'],
            'session_type': session['session_type'],
            'title': session.get('title'),
            'model': session.get('model'),
            'input_tokens': session.get('input_tokens', 0),
            'output_tokens': session.get('output_tokens', 0),
            'cost_usd': session.get('cost_usd', 0),
        }, timeout=5)
        return resp.status_code in (200, 201)
    except Exception as e:
        print(f"  Error posting session: {e}")
        return False

def main():
    import argparse
    parser = argparse.ArgumentParser(description='Parse historical OpenClaw logs')
    parser.add_argument('--hours', type=int, help='Hours of history to parse')
    parser.add_argument('--days', type=int, default=7, help='Days of history to parse (default: 7)')
    parser.add_argument('--dry-run', action='store_true', help='Print what would be posted without posting')
    parser.add_argument('--events-only', action='store_true', help='Only process events, skip sessions')
    parser.add_argument('--sessions-only', action='store_true', help='Only process sessions, skip events')
    args = parser.parse_args()
    
    if not LOG_PATH.exists():
        print(f"Log file not found: {LOG_PATH}")
        sys.exit(1)
    
    # Calculate cutoff time
    if args.hours:
        cutoff = datetime.now(timezone.utc) - timedelta(hours=args.hours)
        period_desc = f"{args.hours} hours"
    else:
        cutoff = datetime.now(timezone.utc) - timedelta(days=args.days)
        period_desc = f"{args.days} days"
    
    print(f"Parsing logs from the last {period_desc} (since {cutoff.isoformat()})")
    
    events_count = 0
    sessions_count = 0
    lines_processed = 0
    events_to_post = []
    
    with open(LOG_PATH, 'r') as f:
        for line in f:
            lines_processed += 1
            if lines_processed % 50000 == 0:
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
                msg = str(data.get('0', ''))
            else:
                msg = str(msg)
            
            # Extract model if present
            model = extract_field(msg, 'model=')
            
            # Process session state (always, for tracking)
            if not args.events_only:
                process_session(msg, time_str, model)
            
            # Categorize and collect event
            if not args.sessions_only:
                event = categorize_event(msg, subsystem, level, time_str)
                if event:
                    events_to_post.append(event)
    
    # Finalize sessions
    if not args.events_only:
        finalize_sessions()
    
    print(f"\nProcessed {lines_processed} lines")
    print(f"Found {len(events_to_post)} events and {len(completed_sessions)} sessions")
    
    # Post events (limit to avoid overwhelming)
    if not args.sessions_only:
        print(f"\nPosting events...")
        # Deduplicate and limit events
        seen_summaries = set()
        unique_events = []
        for event in events_to_post:
            key = f"{event['event_type']}:{event['summary'][:50]}"
            if key not in seen_summaries:
                seen_summaries.add(key)
                unique_events.append(event)
        
        # Post most recent 500 events
        for event in unique_events[-500:]:
            if post_event(event, args.dry_run):
                events_count += 1
    
    # Post sessions
    if not args.events_only:
        print(f"\nPosting sessions...")
        for session in completed_sessions:
            if post_session(session, args.dry_run):
                sessions_count += 1
    
    print(f"\nDone! Posted {events_count} events and {sessions_count} sessions")

if __name__ == '__main__':
    main()
