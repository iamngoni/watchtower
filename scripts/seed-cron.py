#!/usr/bin/env python3
"""
Seed Watchtower with example cron job data.
Customize the CRON_JOBS list for your setup.
"""

import json
import urllib.request
import urllib.error

WATCHTOWER_URL = "http://localhost:3002/api/cron/sync"

# Example cron jobs — replace with your own
CRON_JOBS = {
    "jobs": [
        {
            "job_id": "example-morning-briefing",
            "name": "Morning Briefing",
            "schedule": "0 8 * * *",
            "enabled": True,
        },
        {
            "job_id": "example-daily-backup",
            "name": "Daily Backup",
            "schedule": "0 3 * * *",
            "enabled": True,
        },
        {
            "job_id": "example-health-check",
            "name": "Health Check",
            "schedule": "*/15 * * * *",
            "enabled": True,
        },
    ]
}


def main():
    data = json.dumps(CRON_JOBS).encode("utf-8")
    req = urllib.request.Request(
        WATCHTOWER_URL,
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    try:
        with urllib.request.urlopen(req) as resp:
            result = json.loads(resp.read())
            print(f"Synced {len(result)} cron jobs")
            for job in result:
                print(f"  - {job['name']} ({job['schedule']})")
    except urllib.error.URLError as e:
        print(f"Error: {e}")


if __name__ == "__main__":
    main()
