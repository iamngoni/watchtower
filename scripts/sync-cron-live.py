#!/usr/bin/env python3
"""
Sync OpenClaw cron job status to Watchtower.

The agent pipes OpenClaw cron list data via stdin:
  openclaw cron list --json | python3 sync-cron-live.py

Or pass a JSON file:
  python3 sync-cron-live.py --file cron-data.json
"""

import json
import sys
import requests
from datetime import datetime, timezone

API_BASE = "http://localhost:3002/api"

def parse_timestamp(ts_str):
    """Parse timestamp string to Unix timestamp."""
    if not ts_str:
        return None
    try:
        # Try ISO format
        dt = datetime.fromisoformat(ts_str.replace('Z', '+00:00'))
        return int(dt.timestamp())
    except:
        pass
    try:
        # Try Unix timestamp
        return int(float(ts_str))
    except:
        pass
    return None

def map_cron_job(openclaw_job):
    """Map OpenClaw cron job to Watchtower format."""
    return {
        'job_id': openclaw_job.get('id') or openclaw_job.get('job_id') or openclaw_job.get('name', 'unknown'),
        'name': openclaw_job.get('name') or openclaw_job.get('label') or openclaw_job.get('id', 'Unnamed'),
        'schedule': openclaw_job.get('schedule') or openclaw_job.get('cron') or '* * * * *',
        'enabled': openclaw_job.get('enabled', True),
        'last_status': map_status(openclaw_job.get('lastStatus') or openclaw_job.get('last_status')),
        'last_run_at': parse_timestamp(openclaw_job.get('lastRunAt') or openclaw_job.get('last_run_at') or openclaw_job.get('lastRun')),
        'next_run_at': parse_timestamp(openclaw_job.get('nextRunAt') or openclaw_job.get('next_run_at') or openclaw_job.get('nextRun')),
        'consecutive_errors': openclaw_job.get('consecutiveErrors') or openclaw_job.get('consecutive_errors') or 0,
    }

def map_status(status):
    """Normalize status string."""
    if not status:
        return None
    status_lower = status.lower()
    if 'success' in status_lower or 'ok' in status_lower or 'completed' in status_lower:
        return 'success'
    if 'error' in status_lower or 'fail' in status_lower:
        return 'error'
    if 'running' in status_lower or 'progress' in status_lower:
        return 'running'
    if 'skip' in status_lower:
        return 'skipped'
    return status

def sync_jobs(jobs, dry_run=False):
    """Sync jobs to Watchtower API."""
    mapped_jobs = [map_cron_job(j) for j in jobs]
    
    if dry_run:
        print("Would sync the following jobs:")
        for job in mapped_jobs:
            print(f"  - {job['job_id']}: {job['name']} ({job['schedule']}) - {job.get('last_status', 'unknown')}")
        return True
    
    try:
        resp = requests.post(
            f"{API_BASE}/cron/sync",
            json={'jobs': mapped_jobs},
            timeout=10
        )
        if resp.status_code in (200, 201):
            result = resp.json()
            print(f"✓ Synced {len(result)} cron jobs")
            return True
        else:
            print(f"✗ Sync failed: {resp.status_code} - {resp.text}")
            return False
    except Exception as e:
        print(f"✗ Error syncing: {e}")
        return False

def main():
    import argparse
    parser = argparse.ArgumentParser(description='Sync OpenClaw cron status to Watchtower')
    parser.add_argument('--file', '-f', help='Read JSON from file instead of stdin')
    parser.add_argument('--dry-run', '-n', action='store_true', help='Show what would be synced')
    args = parser.parse_args()
    
    # Read input
    if args.file:
        with open(args.file, 'r') as f:
            data = f.read()
    else:
        if sys.stdin.isatty():
            print("Usage: openclaw cron list --json | python3 sync-cron-live.py")
            print("   or: python3 sync-cron-live.py --file cron-data.json")
            sys.exit(1)
        data = sys.stdin.read()
    
    # Parse JSON
    try:
        parsed = json.loads(data)
    except json.JSONDecodeError as e:
        print(f"Error parsing JSON: {e}")
        sys.exit(1)
    
    # Handle various input formats
    if isinstance(parsed, list):
        jobs = parsed
    elif isinstance(parsed, dict):
        # Could be { jobs: [...] } or { cron: [...] } or single job
        jobs = parsed.get('jobs') or parsed.get('cron') or parsed.get('items') or [parsed]
    else:
        print(f"Unexpected data format: {type(parsed)}")
        sys.exit(1)
    
    if not jobs:
        print("No cron jobs found in input")
        sys.exit(0)
    
    print(f"Found {len(jobs)} cron jobs to sync")
    sync_jobs(jobs, args.dry_run)

if __name__ == '__main__':
    main()
