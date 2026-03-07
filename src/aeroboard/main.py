import click
import subprocess
import os
import sys
import signal

@click.group()
def main():
    """Aeroboard: Arrow-Native Real-Time ML Dashboard CLI"""
    pass

@main.command()
def setup():
    """Build the frontend and prepare the server"""
    # Use the current file path to find the project root
    # src/aeroboard/main.py -> aeroboard/
    root_dir = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    web_dir = os.path.join(root_dir, "aeroboard-web")
    server_dir = os.path.join(root_dir, "aeroboard-server")

    click.echo("--- 🏗️ Building Aeroboard Web ---")
    if os.path.exists(web_dir):
        subprocess.run(["npm", "install"], cwd=web_dir, check=True)
        subprocess.run(["npm", "run", "build"], cwd=web_dir, check=True)
    else:
        click.echo(f"Error: web directory not found at {web_dir}", err=True)
        sys.exit(1)

    click.echo("\n--- 🦀 Building Aeroboard Server ---")
    if os.path.exists(server_dir):
        # We build in release mode for performance
        subprocess.run(["cargo", "build", "--release"], cwd=server_dir, check=True)
    else:
        click.echo(f"Error: server directory not found at {server_dir}", err=True)
        sys.exit(1)
    
    click.echo("\n✅ Setup complete! You can now run 'aeroboard start'")

@main.command()
@click.option("--port", default=3000, help="Port to run the server on")
@click.pass_context
def start(ctx, port):
    """Start the Aeroboard server (and web UI)"""
    # Use the current file path to find the project root
    root_dir = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    server_dir = os.path.join(root_dir, "aeroboard-server")
    binary_path = os.path.join(server_dir, "target", "release", "aeroboard")
    
    # Check if web build exists
    dist_dir = os.path.join(root_dir, "aeroboard-web", "dist")
    if not os.path.exists(dist_dir):
        click.echo("⚠️ Warning: Frontend build not found. Running setup first...")
        ctx.invoke(setup)

    # Check if binary exists
    if not os.path.exists(binary_path):
        click.echo("⚠️ Warning: Release binary not found. Running setup first...")
        ctx.invoke(setup)

    click.echo(f"🚀 Starting Aeroboard on http://localhost:{port}")
    click.echo("   (Serving metrics stream and live dashboard)")
    
    try:
        # Run the compiled binary directly
        subprocess.run([binary_path], cwd=server_dir)
    except FileNotFoundError:
        # Fallback to cargo run if for some reason binary_path didn't work but we are in dev
        click.echo("⚠️ Release binary not found, falling back to 'cargo run'...")
        subprocess.run(["cargo", "run", "--release"], cwd=server_dir)
    except KeyboardInterrupt:
        click.echo("\n👋 Shutting down...")

@main.command()
def db():
    """Start GreptimeDB via Docker Compose"""
    root_dir = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    click.echo("Starting GreptimeDB...")
    subprocess.run(["docker-compose", "up", "-d"], cwd=root_dir, check=True)
    click.echo("GreptimeDB is running.")

if __name__ == "__main__":
    main()
