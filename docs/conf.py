"""Sphinx configuration for the Aeroboard documentation."""

import os
from pathlib import Path
import re
import tomllib
import unicodedata

from docutils import nodes


project = "Aeroboard"
author = "Aeroboard Team"
release = tomllib.loads(
    (Path(__file__).resolve().parents[1] / "pyproject.toml").read_text(encoding="utf-8")
)["project"]["version"]

extensions = ["myst_parser", "sphinx_rtd_theme"]
source_suffix = {".md": "markdown"}
root_doc = "index"
exclude_patterns = ["_build", "Thumbs.db", ".DS_Store"]
nitpicky = True

# Preserve links to Markdown headings such as #initial-dashboard-view.
myst_heading_anchors = 6
myst_enable_extensions = ["deflist"]

html_theme = "sphinx_rtd_theme"
html_title = "Aeroboard Documentation"
html_baseurl = os.environ.get("READTHEDOCS_CANONICAL_URL", "https://aeroboard.readthedocs.io/")
html_theme_options = {"navigation_depth": 4}


def preserve_heading_anchors(_app, doctree):
    """Keep published Markdown fragment URLs alongside Sphinx's section IDs."""
    used = set()
    for section in doctree.findall(nodes.section):
        title = section[0].astext()
        normalized = unicodedata.normalize("NFKD", title).encode("ascii", "ignore").decode()
        base = re.sub(r"[-\s]+", "-", re.sub(r"[^\w\s-]", "", normalized).strip().lower())
        anchor = base
        suffix = 1
        while anchor in used:
            anchor = f"{base}_{suffix}"
            suffix += 1
        used.add(anchor)
        if anchor and anchor not in doctree.ids:
            section["ids"].append(anchor)
            doctree.ids[anchor] = section


def setup(app):
    app.connect("doctree-read", preserve_heading_anchors)
