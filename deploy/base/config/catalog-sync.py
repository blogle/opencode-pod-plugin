#!/usr/bin/env python3
"""Create lightweight central OpenCode project seeds from the project registry."""

import pathlib
import re
import subprocess
import sys

import yaml


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: catalog-sync.py PROJECTS_YAML CATALOG_DIRECTORY")
    projects_file = pathlib.Path(sys.argv[1])
    catalog = pathlib.Path(sys.argv[2])
    projects = yaml.safe_load(projects_file.read_text(encoding="utf-8"))["projects"]
    catalog.mkdir(parents=True, exist_ok=True)
    for key, project in sorted(projects.items()):
        if not re.fullmatch(r"[A-Za-z0-9_-]+", key):
            raise SystemExit(f"invalid project key: {key}")
        destination = catalog / key
        if destination.exists():
            continue
        subprocess.run(
            [
                "git",
                "clone",
                "--depth=1",
                "--filter=blob:none",
                "--single-branch",
                "--branch",
                str(project["defaultRef"]),
                str(project["repository"]),
                str(destination),
            ],
            check=True,
        )


if __name__ == "__main__":
    main()
