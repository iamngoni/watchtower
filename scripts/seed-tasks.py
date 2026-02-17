#!/usr/bin/env python3
"""
Seed Watchtower with initial task data.
"""

import json
import urllib.request
import urllib.error

WATCHTOWER_URL = "http://localhost:3002/api/tasks"

TASKS = [
    {
        "title": "Add /health endpoint to Kompressor",
        "description": "Uptime Kuma getting 404 on Kompressor health check",
        "priority": "normal",
        "status": "todo",
        "labels": ["kompressor", "monitoring"],
        "created_by": "agent",
        "assigned_to": "agent"
    },
    {
        "title": "Fix Jellyfin Daily Backup timeout",
        "description": "Cron job failing with 'job execution timed out' (120s timeout). consecutiveErrors: 4. Need to increase timeout or optimize the backup script.",
        "priority": "high",
        "status": "todo",
        "labels": ["jellyfin", "backup", "cron"],
        "created_by": "agent",
        "assigned_to": "agent"
    },
    {
        "title": "Fix Morning Briefing delivery",
        "description": "Cron announce delivery failing. consecutiveErrors: 1.",
        "priority": "high",
        "status": "todo",
        "labels": ["cron", "telegram"],
        "created_by": "agent",
        "assigned_to": "agent"
    },
    {
        "title": "Remove Tdarr monitor from Uptime Kuma",
        "description": "Tdarr was removed from the stack but monitor still exists in Uptime Kuma",
        "priority": "low",
        "status": "backlog",
        "labels": ["monitoring", "cleanup"],
        "created_by": "agent",
        "assigned_to": "agent"
    },
    {
        "title": "Get extra drive for media server",
        "description": "Media drive at 42GB free (3.7TB total, 87% full). Downloads paused until extra drive arrives. Sonarr/Radarr stopped, qBit paused.",
        "priority": "high",
        "status": "blocked",
        "labels": ["hardware", "storage"],
        "created_by": "human",
        "assigned_to": "human"
    },
    {
        "title": "Research African Payments SDK",
        "description": "Unified SDK for Paystack, Flutterwave, M-Pesa, Paynow, Pesepay, EcoCash. $49-199/month SaaS. $40B+ market growing 20%/year.",
        "priority": "normal",
        "status": "backlog",
        "labels": ["business", "payments"],
        "created_by": "human",
        "assigned_to": "human"
    },
    {
        "title": "Sign up for technical writing programs",
        "description": "Draft.dev ($315-578), Twilio ($650), CircleCI ($350-600), Vonage ($500), Bugfender ($500), Smashing Magazine ($200-250)",
        "priority": "normal",
        "status": "backlog",
        "labels": ["business", "writing"],
        "created_by": "human",
        "assigned_to": "human"
    },
    {
        "title": "Monitor Kompressor progress",
        "description": "Currently ~2 GB/day savings, ~4,282 files remaining. 725 processed (723 saved, 2 skipped), 18.1 GB saved total.",
        "priority": "normal",
        "status": "in_progress",
        "labels": ["kompressor", "monitoring"],
        "created_by": "agent",
        "assigned_to": "agent"
    },
    {
        "title": "Download shows when extra drive arrives",
        "description": "Slow Horses, Day of the Jackal, Goliath: The Final Fight. Also re-download: Hobbit Extended (15GB), Dragon's Den (~23GB), Shark Tank (~4.7GB)",
        "priority": "normal",
        "status": "blocked",
        "labels": ["media", "downloads"],
        "created_by": "human",
        "assigned_to": "agent"
    }
]

def main():
    created = 0
    errors = 0
    
    for task in TASKS:
        try:
            req = urllib.request.Request(
                WATCHTOWER_URL,
                data=json.dumps(task).encode('utf-8'),
                headers={'Content-Type': 'application/json'},
                method='POST'
            )
            
            with urllib.request.urlopen(req, timeout=10) as response:
                result = json.loads(response.read().decode('utf-8'))
                print(f"  ✓ Created: {task['title'][:50]}")
                created += 1
                
        except urllib.error.HTTPError as e:
            body = e.read().decode('utf-8')
            print(f"  ✗ Failed: {task['title'][:40]} - HTTP {e.code}")
            errors += 1
        except urllib.error.URLError as e:
            print(f"  ✗ Connection error: {e.reason}")
            errors += 1
    
    print(f"\n✓ Seeded {created} tasks ({errors} errors)")
    return 0 if errors == 0 else 1

if __name__ == "__main__":
    exit(main())
