#!/bin/bash
set -e

if ! command -v uv >/dev/null 2>&1; then
    echo "❌ uv is required to manage the Aeroboard Python environment."
    echo "   Install uv first, then rerun ./setup.sh"
    exit 1
fi

echo "🌱 Syncing Aeroboard Python environment with uv..."
uv sync --extra examples --dev

echo "📦 Setting up Web and Server components..."
# Use uv run so the CLI executes inside the managed uv environment.
uv run aeroboard setup

echo ""
echo "✅ All set! To start Aeroboard, run:"
echo "   uv run aeroboard start"
echo ""
echo "   To start the database:"
echo "   uv run aeroboard db"
