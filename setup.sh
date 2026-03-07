#!/bin/bash
set -e

# Ensure we are in a virtual environment if uv is used
if [ -d ".venv" ]; then
    echo "🏗️ Using existing virtual environment..."
else
    echo "🌱 Creating virtual environment..."
    uv venv
fi

echo "🚀 Installing Aeroboard Package and Example Dependencies..."
uv pip install -e ".[examples]"

echo "📦 Setting up Web and Server components..."
# Use uv run to ensure we use the installed CLI in the venv
uv run aeroboard setup

echo ""
echo "✅ All set! To start Aeroboard, run:"
echo "   uv run aeroboard start"
echo ""
echo "   To start the database:"
echo "   uv run aeroboard db"
