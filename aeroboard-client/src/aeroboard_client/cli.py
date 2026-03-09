"""CLI commands for Aeroboard."""

from __future__ import annotations

import os
import subprocess
import sys

import click

from aeroboard_client.runtime import (
    RuntimeAssetError,
    ensure_server_binary,
    ensure_web_dist,
    release_tag,
    start_greptime_container,
    stop_greptime_container,
)


@click.group()
def main():
    """
    Aeroboard: Arrow-Native Real-Time ML Dashboard CLI.

    This CLI provides tools for setting up and running the Aeroboard
    real-time ML dashboard system.
    """
    pass


def _prepare_runtime_assets(build_from_source: bool, force_download: bool) -> tuple[str, str]:
    try:
        web_dist_dir = ensure_web_dist(
            prefer_local_build=build_from_source,
            force_download=force_download,
            reporter=click.echo,
        )
        binary_path = ensure_server_binary(
            prefer_local_build=build_from_source,
            force_download=force_download,
            reporter=click.echo,
        )
    except RuntimeAssetError as exc:
        click.echo(f"Error: {exc}", err=True)
        click.echo(
            f"Aeroboard {release_tag()} needs release artifacts or a local Rust/Node toolchain.",
            err=True,
        )
        sys.exit(1)
    return str(binary_path), str(web_dist_dir)


@main.command()
@click.option(
    "--build-from-source",
    is_flag=True,
    help="Build the frontend and server locally instead of downloading release artifacts.",
)
@click.option(
    "--force-download",
    is_flag=True,
    help="Redownload the packaged runtime artifacts even when local assets exist.",
)
def setup(build_from_source, force_download):
    """
    Prepare the Aeroboard runtime assets.

    By default this downloads the prebuilt web bundle and platform-specific server
    binary from the matching GitHub release. In a source checkout you can opt into
    a full local build with --build-from-source.
    """
    binary_path, web_dist_dir = _prepare_runtime_assets(build_from_source, force_download)
    click.echo(f"✅ Server binary ready at {binary_path}")
    click.echo(f"✅ Web bundle ready at {web_dist_dir}")
    click.echo("\nRun 'aeroboard start' to launch the dashboard.")


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
@click.option(
    "--build-from-source",
    is_flag=True,
    help="Prefer local npm/cargo builds when running from a source checkout.",
)
@click.option(
    "--force-download",
    is_flag=True,
    help="Refresh the downloaded runtime artifacts before starting the server.",
)
def start(port, flight_port, storage, data_dir, build_from_source, force_download):
    """
    Start the Aeroboard server and web UI.

    Args:
        port: HTTP port for serving the web UI (default: 3000)
        flight_port: gRPC Flight port for telemetry ingestion (default: 50051)
        storage: Storage backend - 'filesystem', 'greptime', or 'live' (default: filesystem)
        data_dir: Data directory for filesystem storage
    """
    click.echo("--- 🚀 Starting Aeroboard Server ---")
    binary_path, web_dist_dir = _prepare_runtime_assets(build_from_source, force_download)

    click.echo(f"📍 HTTP UI:     http://localhost:{port}")
    click.echo(f"📍 Flight API:  grpc://localhost:{flight_port}")
    click.echo(f"📍 Storage:     {storage}")
    click.echo("   (Serving metrics stream and live dashboard)")

    env = os.environ.copy()
    env["AEROBOARD_HTTP_PORT"] = str(port)
    env["AEROBOARD_FLIGHT_PORT"] = str(flight_port)
    env["AEROBOARD_STORAGE"] = storage.lower()
    env["AEROBOARD_WEB_DIST_DIR"] = web_dist_dir
    if data_dir:
        env["AEROBOARD_DATA_DIR"] = os.path.abspath(data_dir)

    try:
        result = subprocess.run([binary_path], cwd=os.path.dirname(binary_path), env=env, check=False)
        if result.returncode != 0:
            click.echo(f"Error: Aeroboard server exited with code {result.returncode}", err=True)
            sys.exit(result.returncode)
    except KeyboardInterrupt:
        click.echo("\n👋 Shutting down...")


@main.command()
@click.option("--greptime-image", default="greptime/greptimedb:latest", help="GreptimeDB Docker image")
def db(greptime_image):
    """
    Start GreptimeDB in Docker.

    This command starts the GreptimeDB instance used for database-backed storage.
    Use 'aeroboard stop-db' to stop the database.
    """
    click.echo("--- 🗄️ Starting GreptimeDB ---")
    click.echo(f"Image: {greptime_image}")
    subprocess.run(["docker", "info"], check=True, capture_output=True)
    start_greptime_container(greptime_image, reporter=click.echo)
    click.echo("✅ GreptimeDB is running.")


@main.command()
def stop_db():
    """Stop the managed GreptimeDB Docker container."""
    click.echo("--- ⏸️ Stopping GreptimeDB ---")
    stopped = stop_greptime_container(reporter=click.echo)
    if stopped:
        click.echo("✅ GreptimeDB stopped.")
    else:
        click.echo("ℹ️ GreptimeDB container was not running.")


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
@click.option("--no-build", is_flag=True, help="Skip preparing runtime assets before checking the run")
def check_run(run_id=None, no_build=False):
    """
    Check the status of a specific run.

    Args:
        run_id: The run ID to check (or latest run if not specified)
        no_build: Skip preparing runtime assets first
    """
    if not no_build:
        _prepare_runtime_assets(build_from_source=False, force_download=False)
    
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