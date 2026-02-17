#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

echo "=== Building Watchtower Docker Image ==="
echo "Project directory: $PROJECT_DIR"

# Build the Docker image
docker build -t watchtower:latest .

echo ""
echo "=== Build Complete ==="
echo "Image: watchtower:latest"
echo ""
echo "To run locally:"
echo "  docker run -p 3002:3002 -v \$(pwd)/data:/data watchtower:latest"
echo ""
echo "To add to your stack, see docker-compose.snippet.yml"
