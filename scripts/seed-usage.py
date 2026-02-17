#!/usr/bin/env python3
"""
Seed usage data for Watchtower costs page.
Generates realistic usage data based on typical Claude usage patterns.

Usage: ./seed-usage.py [--days N]
"""

import requests
import random
from datetime import datetime, timedelta

API_BASE = "http://localhost:3002/api"

# Claude pricing (approximate, per 1M tokens)
PRICING = {
    'claude-opus-4-6': {'input': 15.0, 'output': 75.0},
    'claude-opus-4-5': {'input': 15.0, 'output': 75.0},
    'claude-sonnet-4-5': {'input': 3.0, 'output': 15.0},
    'claude-3-haiku': {'input': 0.25, 'output': 1.25},
}

def calculate_cost(model, input_tokens, output_tokens):
    """Calculate cost in USD."""
    pricing = PRICING.get(model, PRICING['claude-opus-4-5'])
    input_cost = (input_tokens / 1_000_000) * pricing['input']
    output_cost = (output_tokens / 1_000_000) * pricing['output']
    return round(input_cost + output_cost, 4)

def generate_day_usage(date):
    """Generate realistic usage for a day."""
    usage = []
    
    # Typical day has 5-20 agent interactions
    num_interactions = random.randint(5, 20)
    
    # Use opus most of the time for main agent
    models = ['claude-opus-4-5'] * 7 + ['claude-opus-4-6'] * 2 + ['claude-sonnet-4-5']
    
    for _ in range(num_interactions):
        model = random.choice(models)
        
        # Typical token counts
        if 'opus' in model:
            input_tokens = random.randint(8000, 50000)
            output_tokens = random.randint(500, 5000)
        else:
            input_tokens = random.randint(3000, 15000)
            output_tokens = random.randint(200, 2000)
        
        cost = calculate_cost(model, input_tokens, output_tokens)
        
        usage.append({
            'date': date,
            'model': model,
            'input_tokens': input_tokens,
            'output_tokens': output_tokens,
            'cost_usd': cost,
        })
    
    return usage

def post_usage(usage_data):
    """Post usage to API."""
    try:
        resp = requests.post(f"{API_BASE}/usage/report", json=usage_data, timeout=5)
        return resp.status_code in (200, 201)
    except Exception as e:
        print(f"Error posting usage: {e}")
        return False

def main():
    import argparse
    parser = argparse.ArgumentParser(description='Seed Watchtower usage data')
    parser.add_argument('--days', type=int, default=14, help='Days of history to seed (default: 14)')
    args = parser.parse_args()
    
    print(f"Seeding {args.days} days of usage data...")
    
    total_cost = 0
    total_interactions = 0
    
    for i in range(args.days, 0, -1):
        date = (datetime.now() - timedelta(days=i)).strftime('%Y-%m-%d')
        day_usage = generate_day_usage(date)
        
        day_cost = 0
        for usage in day_usage:
            if post_usage(usage):
                total_interactions += 1
                day_cost += usage['cost_usd']
        
        total_cost += day_cost
        print(f"  {date}: {len(day_usage)} interactions, ${day_cost:.2f}")
    
    print(f"\nDone! Seeded {total_interactions} usage records")
    print(f"Total simulated cost: ${total_cost:.2f}")

if __name__ == '__main__':
    main()
