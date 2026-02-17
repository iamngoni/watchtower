"""
Model pricing per 1M tokens (USD) — shared across scripts.
Last updated: Feb 2026. Prices change — verify at provider sites.
"""

MODEL_PRICING = {
    # Anthropic Claude
    'claude-opus-4-6':      {'input': 15.0,  'output': 75.0},
    'claude-opus-4-5':      {'input': 15.0,  'output': 75.0},
    'claude-sonnet-4-5':    {'input': 3.0,   'output': 15.0},
    'claude-sonnet-4':      {'input': 3.0,   'output': 15.0},
    'claude-3-opus':        {'input': 15.0,  'output': 75.0},
    'claude-3-5-sonnet':    {'input': 3.0,   'output': 15.0},
    'claude-3-sonnet':      {'input': 3.0,   'output': 15.0},
    'claude-3-haiku':       {'input': 0.25,  'output': 1.25},
    'claude-3-5-haiku':     {'input': 0.80,  'output': 4.0},

    # OpenAI
    'gpt-4o':               {'input': 2.50,  'output': 10.0},
    'gpt-4o-mini':          {'input': 0.15,  'output': 0.60},
    'gpt-4-turbo':          {'input': 10.0,  'output': 30.0},
    'gpt-4':                {'input': 30.0,  'output': 60.0},
    'gpt-3.5-turbo':        {'input': 0.50,  'output': 1.50},
    'o1':                   {'input': 15.0,  'output': 60.0},
    'o1-mini':              {'input': 3.0,   'output': 12.0},
    'o1-pro':               {'input': 150.0, 'output': 600.0},
    'o3':                   {'input': 10.0,  'output': 40.0},
    'o3-mini':              {'input': 1.10,  'output': 4.40},
    'o4-mini':              {'input': 1.10,  'output': 4.40},
    'codex-mini':           {'input': 1.50,  'output': 6.0},

    # Google
    'gemini-2.5-pro':       {'input': 1.25,  'output': 10.0},
    'gemini-2.5-flash':     {'input': 0.15,  'output': 0.60},
    'gemini-2.0-flash':     {'input': 0.10,  'output': 0.40},
    'gemini-1.5-pro':       {'input': 1.25,  'output': 5.0},
    'gemini-1.5-flash':     {'input': 0.075, 'output': 0.30},

    # xAI
    'grok-3':               {'input': 3.0,   'output': 15.0},
    'grok-3-mini':          {'input': 0.30,  'output': 0.50},
    'grok-2':               {'input': 2.0,   'output': 10.0},

    # DeepSeek
    'deepseek-r1':          {'input': 0.55,  'output': 2.19},
    'deepseek-v3':          {'input': 0.27,  'output': 1.10},
    'deepseek-chat':        {'input': 0.27,  'output': 1.10},

    # Meta (via API providers)
    'llama-3.1-405b':       {'input': 3.0,   'output': 3.0},
    'llama-3.1-70b':        {'input': 0.88,  'output': 0.88},
    'llama-3.1-8b':         {'input': 0.05,  'output': 0.08},
    'llama-4-maverick':     {'input': 0.50,  'output': 0.77},
    'llama-4-scout':        {'input': 0.17,  'output': 0.27},

    # Mistral
    'mistral-large':        {'input': 2.0,   'output': 6.0},
    'mistral-medium':       {'input': 2.7,   'output': 8.1},
    'mistral-small':        {'input': 0.20,  'output': 0.60},
    'codestral':            {'input': 0.30,  'output': 0.90},
}

# Default pricing for unknown models
DEFAULT_PRICING = {'input': 3.0, 'output': 15.0}


def get_model_cost(model_name, input_tokens, output_tokens):
    """Calculate cost in USD for a given model and token counts."""
    model_lower = (model_name or '').lower()

    # Try exact match first, then substring
    for key, pricing in MODEL_PRICING.items():
        if key in model_lower:
            input_cost = (input_tokens / 1_000_000) * pricing['input']
            output_cost = (output_tokens / 1_000_000) * pricing['output']
            return round(input_cost + output_cost, 6)

    # Unknown model — use default
    input_cost = (input_tokens / 1_000_000) * DEFAULT_PRICING['input']
    output_cost = (output_tokens / 1_000_000) * DEFAULT_PRICING['output']
    return round(input_cost + output_cost, 6)
