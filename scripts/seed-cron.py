#!/usr/bin/env python3
"""
Seed Watchtower with initial cron job data.
"""

import json
import urllib.request
import urllib.error

WATCHTOWER_URL = "http://localhost:3002/api/cron/sync"

CRON_JOBS = {
    "jobs": [
        {
            "job_id": "5936fb39-5994-44d4-8e72-5f24c4d7a6c5",
            "name": "Morning Briefing",
            "schedule": "0 8 * * * (Africa/Johannesburg)",
            "enabled": True,
            "last_status": "error",
            "consecutive_errors": 1
        },
        {
            "job_id": "6f0bd089-a8f3-4436-8f90-45f724aed93c",
            "name": "Jellyfin Daily Backup",
            "schedule": "0 3 * * * (Africa/Johannesburg)",
            "enabled": True,
            "last_status": "error",
            "consecutive_errors": 4
        },
        {
            "job_id": "ccdb875a-28b2-4c4f-bf53-b2febd57e128",
            "name": "J. Cole Tickets Reminder",
            "schedule": "2026-02-20 08:45 SAST (one-shot)",
            "enabled": True
        },
        {
            "job_id": "13911539-f497-4dea-872f-d0571fdafd86",
            "name": "Quickmerlin Invoice Reminder",
            "schedule": "0 9 24 * * (Africa/Johannesburg)",
            "enabled": True
        },
        {
            "job_id": "b49e87f5-8a59-4957-89c6-529b11be2b20",
            "name": "qBit Monitor",
            "schedule": "*/15 * * * * (Africa/Johannesburg)",
            "enabled": False
        },
        {
            "job_id": "f68fe615-d651-4f23-bede-a565116c4d05",
            "name": "qBit Cleanup Completed",
            "schedule": "*/10 * * * * (Africa/Johannesburg)",
            "enabled": False
        },
        {
            "job_id": "76a43557-7d03-413d-b2e4-60da05ba1c24",
            "name": "qBit Episode Upgrader",
            "schedule": "20,50 * * * * (Africa/Johannesburg)",
            "enabled": False
        },
        {
            "job_id": "3f55899a-5536-4d43-8e38-f7def9059ce4",
            "name": "qBit Malware Guard",
            "schedule": "5,35 * * * * (Africa/Johannesburg)",
            "enabled": False
        },
        {
            "job_id": "7797ce47-3fa5-4ec0-8696-b61f7f837360",
            "name": "Error Download Cleanup",
            "schedule": "*/10 * * * * (Africa/Johannesburg)",
            "enabled": False
        }
    ]
}

def main():
    try:
        req = urllib.request.Request(
            WATCHTOWER_URL,
            data=json.dumps(CRON_JOBS).encode('utf-8'),
            headers={'Content-Type': 'application/json'},
            method='POST'
        )
        
        with urllib.request.urlopen(req, timeout=10) as response:
            result = json.loads(response.read().decode('utf-8'))
            # API returns list of synced jobs
            if isinstance(result, list):
                print(f"✓ Seeded {len(result)} cron jobs")
            else:
                print(f"✓ Seeded {result.get('synced', 0)} cron jobs")
            
    except urllib.error.HTTPError as e:
        body = e.read().decode('utf-8')
        print(f"HTTP Error {e.code}: {e.reason}")
        print(f"Response: {body}")
        return 1
    except urllib.error.URLError as e:
        print(f"Connection error: {e.reason}")
        return 1
    
    return 0

if __name__ == "__main__":
    exit(main())
