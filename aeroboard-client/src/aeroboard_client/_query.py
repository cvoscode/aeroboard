"""HTTP read helpers shared by client instances."""
from __future__ import annotations
from typing import Any, Dict, List, Optional
from ._errors import AeroboardClientError
DASHBOARD_CONFIG_KEY="__aeroboard_dashboard__"
class QueryClient:
    def query_history(
        self,
        *,
        run_id: Optional[str] = None,
        metric: Optional[str] = None,
        from_timestamp: Optional[float] = None,
        to_timestamp: Optional[float] = None,
        limit: Optional[int] = None,
        cursor: Optional[int] = None,
    ) -> Dict[str, Any]:
        """
        Query historical metrics data from the backend.

        Args:
            run_id: Filter by run ID. Uses client's run_id if not provided.
            metric: Filter by metric name.
            from_timestamp: Start timestamp in Unix seconds (float with decimals for ms).
            to_timestamp: End timestamp in Unix seconds.
            limit: Maximum number of results to return.
            cursor: Pagination cursor from previous query.

        Returns:
            Dictionary containing:
                - "rows": List of metric data points.
                - "run_summaries": Aggregated per-run summaries.
                - "next_cursor": Cursor for next page (None if last page).

        Example:
            >>> response = client.query_history(run_id="my-run", metric="loss")
            >>> for point in response["rows"]:
            ...     print(point["name"], point["value"])

        Raises:
            AeroboardClientError: If HTTP query fails.
        """
        self._ensure_http_client()
        url = f"{self.api_base_url}/api/history"

        params = {}
        if run_id is None:
            run_id = self.run_id
        if run_id:
            params["run_id"] = run_id
        if metric:
            params["metric"] = metric
        if from_timestamp is not None:
            params["start_ms"] = str(int(from_timestamp * 1000))
        if to_timestamp is not None:
            params["end_ms"] = str(int(to_timestamp * 1000))
        if limit is not None:
            params["limit"] = str(limit)
        if cursor is not None:
            params["cursor"] = str(cursor)

        try:
            resp = self._http_client.get(url, params=params, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            return resp.json()
        except Exception as e:
            raise AeroboardClientError(f"Failed to query history: {e}")


    def query_hyperparameters(
        self,
        *,
        run_id: Optional[str] = None,
        key: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[Dict[str, Any]]:
        """
        Query hyperparameter settings for a run.

        Args:
            run_id: Filter by run ID. Uses client's run_id if not provided.
            key: Filter by hyperparameter key.
            limit: Maximum number of results.

        Returns:
            List of hyperparameter entries.
        """
        self._ensure_http_client()

        url = f"{self.api_base_url}/api/runs/summary"

        params = {}
        if run_id is None:
            run_id = self.run_id
        if run_id:
            params["run_id"] = run_id
        if limit is not None:
            params["limit"] = str(limit)

        try:
            resp = self._http_client.get(url, params=params, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            data = resp.json()
            summaries = data.get("run_summaries", [])
            entries: List[Dict[str, Any]] = []
            for summary in summaries:
                summary_run_id = summary.get("run_id")
                for entry in summary.get("hyperparameters", []):
                    if key is not None and entry.get("key") != key:
                        continue
                    if key is None and entry.get("key") == DASHBOARD_CONFIG_KEY:
                        continue  # internal dashboard settings, not a hyperparameter
                    entries.append({
                        "run_id": summary_run_id,
                        "key": entry.get("key"),
                        "value": entry.get("value"),
                        "value_type": entry.get("value_type"),
                    })
            return entries if limit is None else entries[:limit]
        except Exception as e:
            raise AeroboardClientError(f"Failed to query hyperparameters: {e}")


    def query_images(
        self,
        *,
        run_id: Optional[str] = None,
        name: Optional[str] = None,
        format: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[Dict[str, Any]]:
        """
        Query image history for a run.

        Args:
            run_id: Filter by run ID. Uses client's run_id if not provided.
            name: Filter by image name.
            format: Filter by image format (webp, jpeg, png).
            limit: Maximum number of images to return.

        Returns:
            List of image data points with base64-encoded image data.
        """
        self._ensure_http_client()

        url = f"{self.api_base_url}/api/history/images"

        params = {}
        if run_id is None:
            run_id = self.run_id
        if run_id:
            params["run_id"] = run_id
        if name:
            params["metric"] = name
        if limit is not None:
            params["limit"] = str(limit)

        try:
            resp = self._http_client.get(url, params=params, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            data = resp.json()
            rows = data.get("rows", [])
            if format is not None:
                rows = [row for row in rows if row.get("format") == format]
            return rows
        except Exception as e:
            raise AeroboardClientError(f"Failed to query images: {e}")


    def list_runs(self) -> List[str]:
        """
        List all run IDs from the backend.

        Returns:
            List of run ID strings.

        Raises:
            AeroboardClientError: If unable to connect or query.
        """
        self._ensure_http_client()

        url = f"{self.api_base_url}/api/runs"

        try:
            resp = self._http_client.get(url, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            data = resp.json()
            return data.get("runs", [])
        except Exception as e:
            raise AeroboardClientError(f"Failed to list runs: {e}")

