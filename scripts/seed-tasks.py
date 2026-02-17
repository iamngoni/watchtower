#!/usr/bin/env python3
"""
Seed Watchtower with example task data.
Customize the TASKS list for your setup.
"""

import json
import urllib.request
import urllib.error

WATCHTOWER_URL = "http://localhost:3002/api/tasks"

# Example tasks — replace with your own
TASKS = [
    {
        "title": "Set up monitoring dashboard",
        "description": "Configure Watchtower with real data sources and alerts",
        "priority": "high",
        "status": "in_progress",
        "labels": ["setup", "monitoring"],
        "created_by": "human",
        "assigned_to": "agent",
    },
    {
        "title": "Configure Telegram notifications",
        "description": "Set up TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID in .env for alert delivery",
        "priority": "normal",
        "status": "todo",
        "labels": ["notifications"],
        "created_by": "agent",
        "assigned_to": "human",
    },
    {
        "title": "Review agent cost trends",
        "description": "Check the costs page for any unusual spending patterns",
        "priority": "low",
        "status": "backlog",
        "labels": ["costs", "review"],
        "created_by": "agent",
        "assigned_to": "human",
    },
]


def main():
    created = 0
    for task in TASKS:
        data = json.dumps(task).encode("utf-8")
        req = urllib.request.Request(
            WATCHTOWER_URL,
            data=data,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req) as resp:
                result = json.loads(resp.read())
                print(f"  Created: {result['title']} [{result['status']}]")
                created += 1
        except urllib.error.URLError as e:
            print(f"  Error creating '{task['title']}': {e}")

    print(f"\nSeeded {created}/{len(TASKS)} tasks")


if __name__ == "__main__":
    main()
