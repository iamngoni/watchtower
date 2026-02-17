#!/usr/bin/env python3
"""
Sync cron jobs to Watchtower.
Reads JSON from stdin and POSTs to /api/cron/sync

Usage:
    echo '{"jobs": [...]}' | python3 sync-cron.py
    cat cron-data.json | python3 sync-cron.py
"""

import json
import sys
import urllib.request
import urllib.error

WATCHTOWER_URL = "http://localhost:3002/api/cron/sync"

def main():
    try:
        # Read JSON from stdin
        data = json.load(sys.stdin)
    except json.JSONDecodeError as e:
        print(f"Error parsing JSON: {e}", file=sys.stderr)
        sys.exit(1)
    
    # Ensure data has 'jobs' key
    if "jobs" not in data:
        data = {"jobs": data if isinstance(data, list) else []}
    
    try:
        # POST to Watchtower
        req = urllib.request.Request(
            WATCHTOWER_URL,
            data=json.dumps(data).encode('utf-8'),
            headers={'Content-Type': 'application/json'},
            method='POST'
        )
        
        with urllib.request.urlopen(req, timeout=10) as response:
            result = json.loads(response.read().decode('utf-8'))
            if isinstance(result, list):
                print(f"✓ Synced {len(result)} cron jobs")
            else:
                print(f"✓ Synced {result.get('synced', 0)} cron jobs")
            
    except urllib.error.HTTPError as e:
        print(f"HTTP Error {e.code}: {e.reason}", file=sys.stderr)
        sys.exit(1)
    except urllib.error.URLError as e:
        print(f"Connection error: {e.reason}", file=sys.stderr)
        sys.exit(1)

if __name__ == "__main__":
    main()
