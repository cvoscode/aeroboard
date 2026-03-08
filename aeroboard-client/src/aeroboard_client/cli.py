"""
CLI commands for Aeroboard.

This module provides the command-line interface for:
- Building the frontend and server
- Starting the Aeroboard server
- Managing the GreptimeDB instance
"""

import subprocess
import os
import sys
import click


@click.group()
def main():
    """
    Aeroboard: Arrow-Native Real-Time ML Dashboard CLI.

    This CLI provides tools for setting up and running the Aeroboard
    real-time ML dashboard system.
    """
    pass


def _project_root() -> str:
    """
    Get the project root directory.

    Returns:
        The absolute path to the project root directory.
    """
    return os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))


def _build_frontend(root_dir: str) -> None:
    """
    Build the Aeroboard web frontend.

    Args:
        root_dir: The root directory of the project.

    Raises:
        SystemExit: If the web directory is not found.
    """
    web_dir = os.path.join(root_dir, "aeroboard-web")
    click.echo("--- 🏗️ Building Aeroboard Web Frontend ---")
    if os.path.exists(web_dir):
        click.echo(f"Installing dependencies and building frontend...")
        subprocess.run(["npm", "install"], cwd=web_dir, check=True, capture_output=True)
        subprocess.run(["npm", "run", "build"], cwd=web_dir, check=True, capture_output=True)
        click.echo("✅ Web frontend built successfully.")
        return

    click.echo(f"Error: web directory not found at {web_dir}", err=True)
    sys.exit(1)


def _build_server(root_dir: str) -> None:
    """
    Build the Aeroboard Rust server.

    Args:
        root_dir: The root directory of the project.

    Raises:
        SystemExit: If the server directory is not found.
    """
    server_dir = os.path.join(root_dir, "aeroboard-server")
    click.echo("\n--- 🦀 Building Aeroboard Rust Server ---")
    if os.path.exists(server_dir):
        subprocess.run(["cargo", "build", "--release"], cwd=server_dir, check=True, capture_output=True)
        click.echo("✅ Rust server built successfully.")
        return

    click.echo(f"Error: server directory not found at {server_dir}", err=True)
    sys.exit(1)


@main.command()
def setup():
    """
    Build the frontend and prepare the server.

    This command:
    1. Builds the web frontend using npm
    2. Builds the Rust server using cargo

    After running this command, you can use 'aeroboard start' to run the server.
    """
    root_dir = _project_root()
    _build_frontend(root_dir)
    _build_server(root_dir)
    click.echo("\n✅ Setup complete! You can now run 'aeroboard start'")


@main.command()
@click.option("--port", default=3000, help="HTTP port for the web UI")
@click.option("--flight-port", default=50051, help="gRPC Flight port for ingestion")
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
    """
    Start the Aeroboard server and web UI.

    Args:
        port: HTTP port for serving the web UI (default: 3000)
        flight_port: gRPC Flight port for telemetry ingestion (default: 50051)
        storage: Storage backend - 'filesystem', 'greptime', or 'live' (default: filesystem)
        data_dir: Data directory for filesystem storage
    """
    root_dir = _project_root()
    server_dir = os.path.join(root_dir, "aeroboard-server")
    binary_path = os.path.join(server_dir, "target", "release", "aeroboard")

    click.echo("--- 🚀 Starting Aeroboard Server ---")
    
    # Build before starting
    _build_frontend(root_dir)
    _build_server(root_dir)

    if not os.path.exists(binary_path):
        click.echo("Error: Release binary not found after build.", err=True)
        sys.exit(1)

    click.echo(f"📍 HTTP UI:     http://localhost:{port}")
    click.echo(f"📍 Flight API:  grpc://localhost:{flight_port}")
    click.echo(f"📍 Storage:     {storage}")
    click.echo("   (Serving metrics stream and live dashboard)")

    env = os.environ.copy()
    env["AEROBOARD_HTTP_PORT"] = str(port)
    env["AEROBOARD_FLIGHT_PORT"] = str(flight_port)
    env["AEROBOARD_STORAGE"] = storage.lower()
    if data_dir:
        env["AEROBOARD_DATA_DIR"] = os.path.abspath(data_dir)
    
    try:
        result = subprocess.run([binary_path], cwd=server_dir, env=env, check=False)
        if result.returncode != 0:
            click.echo(f"Error: Aeroboard server exited with code {result.returncode}", err=True)
            sys.exit(result.returncode)
    except FileNotFoundError:
        click.echo("⚠️ Release binary not found, falling back to 'cargo run'...")
        result = subprocess.run(["cargo", "run", "--release"], cwd=server_dir, env=env, check=False)
        if result.returncode != 0:
            click.echo(f"Error: Aeroboard cargo server exited with code {result.returncode}", err=True)
            sys.exit(result.returncode)
    except KeyboardInterrupt:
        click.echo("\n👋 Shutting down...")


@main.command()
@click.option("--greptime-image", default="greptime/greptimedb:latest", help="GreptimeDB Docker image")
def db(greptime_image):
    """
    Start GreptimeDB via Docker Compose.

    This command starts the GreptimeDB instance used for database-backed storage.
    Use 'aeroboard stop-db' to stop the database.
    """
    root_dir = _project_root()
    click.echo("--- 🗄️ Starting GreptimeDB ---")
    click.echo(f"Image: {greptime_image}")
    subprocess.run(["docker-compose", "up", "-d"], cwd=root_dir, check=True, capture_output=True)
    click.echo("✅ GreptimeDB is running.")


@main.command()
def stop_db():
    """Stop GreptimeDB via Docker Compose."""
    root_dir = _project_root()
    click.echo("--- ⏸️ Stopping GreptimeDB ---")
    subprocess.run(["docker-compose", "stop"], cwd=root_dir, check=True, capture_output=True)
    click.echo("✅ GreptimeDB stopped.")


@main.command()
@click.argument("run_id", required=False)
def status(run_id=None):
    """
    Show Aeroboard status.

    Args:
        run_id: Optional specific run ID to check status for.
    """
    click.echo("--- 📊 Aeroboard Status ---")
    
    # Check HTTP server
    import requests
    
    try:
        resp = requests.get("http://localhost:3000/api/runs", timeout=2)
        if resp.status_code == 200:
            runs = resp.json().get("runs", [])
            click.echo(f"✅ HTTP server running on port 3000")
            click.echo(f"   Found {len(runs)} run(s):")
            for run in runs[:10]:  # Show first 10
                click.echo(f"   - {run}")
            if len(runs) > 10:
                click.echo(f"   ... and {len(runs) - 10} more")
        else:
            click.echo(f"ℹ️ HTTP server not responding on port 3000")
    except requests.exceptions.ConnectionError:
        click.echo("ℹ️ HTTP server not running on port 3000")
    except Exception as e:
        click.echo(f"⚠️ Error checking HTTP server: {e}")
    
    # Check Flight server
    try:
        from pyarrow.flight import FlightClient
        client = FlightClient("grpc://localhost:50051")
        client.close()
        click.echo("✅ Flight server running on port 50051")
    except Exception as e:
        click.echo(f"ℹ️ Flight server not running: {e}")
    
    # Check database
    try:
        from requests import get
        resp = get("http://localhost:4000/health", timeout=2)
        if resp.status_code == 200:
            click.echo("✅ GreptimeDB is healthy")
        else:
            click.echo("⚠️ GreptimeDB not responding on port 4000")
    except Exception:
        click.echo("ℹ️ GreptimeDB not responding (may not be configured)")


@main.command()
@click.argument("run_id", required=False)
@click.option("--no-build", is_flag=True, help="Skip building")
def check_run(run_id=None, no_build=False):
    """
    Check the status of a specific run.

    Args:
        run_id: The run ID to check (or latest run if not specified)
        no_build: Skip building the project first
    """
    if not no_build:
        root_dir = _project_root()
        _build_frontend(root_dir)
        _build_server(root_dir)
    
    if run_id is None:
        # Get latest run ID from API
        try:
            import requests
            resp = requests.get("http://localhost:3000/api/runs", timeout=2)
            if resp.status_code == 200:
                runs = resp.json().get("runs", [])
                if runs:
                    run_id = runs[0]
                    click.echo(f"Checking run: {run_id}")
                else:
                    click.echo("No runs found.")
                    return
        except Exception as e:
            click.echo(f"Could not determine run ID: {e}")
            return
    
    if run_id:
        click.echo(f"🔍 Run ID: {run_id}")
        click.echo("--- 📈 Metrics ---")
        
        try:
            import requests
            resp = requests.get(f"http://localhost:3000/api/history?run_id={run_id}", timeout=2)
            if resp.status_code == 200:
                data = resp.json()
                points = data.get("rows", [])
                if points:
                    for point in reversed(points[-100:]):
                        name = point.get("name", "unknown")
                        value = point.get("value", 0)
                        click.echo(f"  {name}: {value:.4f}")
                else:
                    click.echo("  No metrics recorded.")
            else:
                click.echo(f"Error fetching metrics: {resp.status_code}")
        except Exception as e:
            click.echo(f"Error: {e}")


@main.command()
@click.option("--port", default=3000)
@click.option("--flight-port", default=50051)
@click.option("--storage", type=click.Choice(["filesystem", "greptime", "live"]), default="filesystem")
@click.argument("run_id", required=False)
@click.option("--metrics", required=False, multiple=True, help="Additional metric to start logging")
def log_run(port, flight_port, storage, run_id=None, metrics=None):
    """
    Start logging metrics to an existing run.

    Args:
        port: HTTP port
        flight_port: gRPC Flight port
        storage: Storage backend
        run_id: Run ID to log to
        metrics: Additional metrics to log (format: name=value)
    """
    click.echo("--- 📝 Logging Run ---")
    click.echo(f"Run ID: {run_id}")
    click.echo(f"Port: {port}, Flight: {flight_port}")
    
    if metrics:
        for metric in metrics:
            if "=" in metric:
                name, value = metric.split("=", 1)
                click.echo(f"Metric to log: {name} = {value}")
    
    click.echo("Tip: Use 'import aeroboard_client as aeroboard' in Python to start logging from your code.")


if __name__ == "__main__":
    main()