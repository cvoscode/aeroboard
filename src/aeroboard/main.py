import click
import subprocess
import os
import sys


def project_root() -> str:
    return os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def build_frontend(root_dir: str) -> None:
    web_dir = os.path.join(root_dir, "aeroboard-web")
    click.echo("--- 🏗️ Building Aeroboard Web ---")
    if os.path.exists(web_dir):
        subprocess.run(["npm", "install"], cwd=web_dir, check=True)
        subprocess.run(["npm", "run", "build"], cwd=web_dir, check=True)
        return

    click.echo(f"Error: web directory not found at {web_dir}", err=True)
    sys.exit(1)


def build_server(root_dir: str) -> None:
    server_dir = os.path.join(root_dir, "aeroboard-server")
    click.echo("\n--- 🦀 Building Aeroboard Server ---")
    if os.path.exists(server_dir):
        subprocess.run(["cargo", "build", "--release"], cwd=server_dir, check=True)
        return

    click.echo(f"Error: server directory not found at {server_dir}", err=True)
    sys.exit(1)

@click.group()
def main():
    """Aeroboard: Arrow-Native Real-Time ML Dashboard CLI"""
    pass

@main.command()
def setup():
    """Build the frontend and prepare the server"""
    root_dir = project_root()
    build_frontend(root_dir)
    build_server(root_dir)
    
    click.echo("\n✅ Setup complete! You can now run 'aeroboard start'")

@main.command()
@click.option("--port", default=3000, help="Port to run the server on")
@click.option("--flight-port", default=50051, help="Port to run the Flight ingestion server on")
@click.option(
    "--storage",
    type=click.Choice(["filesystem", "greptime", "live"], case_sensitive=False),
    default="filesystem",
    show_default=True,
    help="Persistence backend to use",
)
@click.option(
    "--data-dir",
    default=None,
    help="Filesystem data directory used when storage=filesystem",
)
def start(port, flight_port, storage, data_dir):
    """Start the Aeroboard server (and web UI)"""
    root_dir = project_root()
    server_dir = os.path.join(root_dir, "aeroboard-server")
    binary_path = os.path.join(server_dir, "target", "release", "aeroboard")

    build_frontend(root_dir)
    build_server(root_dir)

    if not os.path.exists(binary_path):
        click.echo("Error: Release binary not found after build.", err=True)
        sys.exit(1)

    click.echo(f"🚀 Starting Aeroboard on http://localhost:{port}")
    click.echo(f"   Storage backend: {storage}")
    click.echo(f"   Flight endpoint: grpc://localhost:{flight_port}")
    click.echo("   (Serving metrics stream and live dashboard)")

    env = os.environ.copy()
    env["AEROBOARD_HTTP_PORT"] = str(port)
    env["AEROBOARD_FLIGHT_PORT"] = str(flight_port)
    env["AEROBOARD_STORAGE"] = storage.lower()
    if data_dir:
        env["AEROBOARD_DATA_DIR"] = os.path.abspath(data_dir)
    
    try:
        # Run the compiled binary directly
        result = subprocess.run([binary_path], cwd=server_dir, env=env, check=False)
        if result.returncode != 0:
            click.echo(f"Error: Aeroboard server exited with code {result.returncode}", err=True)
            sys.exit(result.returncode)
    except FileNotFoundError:
        # Fallback to cargo run if for some reason binary_path didn't work but we are in dev
        click.echo("⚠️ Release binary not found, falling back to 'cargo run'...")
        result = subprocess.run(["cargo", "run", "--release"], cwd=server_dir, env=env, check=False)
        if result.returncode != 0:
            click.echo(f"Error: Aeroboard cargo server exited with code {result.returncode}", err=True)
            sys.exit(result.returncode)
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
