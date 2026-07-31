#!/usr/bin/env python3
"""Bounded shared AKE probe for Chimera."""

from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path


SERVICE = "displaylink-driver.service"


def run(command: list[str], *, check: bool = True, timeout: float | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=check, text=True, capture_output=True, timeout=timeout)


def service_active() -> bool:
    return run(["systemctl", "is-active", "--quiet", SERVICE], check=False).returncode == 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ack-display-disruption", action="store_true")
    args = parser.parse_args()
    if not args.ack_display_disruption:
        parser.error("--ack-display-disruption is required")
    if run(["sudo", "-n", "true"], check=False).returncode != 0:
        print("sudo is not authorized in this terminal; run sudo -v first", file=sys.stderr)
        return 1

    root = Path(__file__).resolve().parents[2]
    workspace = root / "revdi"
    binary = workspace / "target/release/chimera-shared-ake-probe"
    evdi_lib = root / "evdi/library"

    build = subprocess.run(
        ["cargo", "build", "--release", "-p", "vino-chimera", "--bin", "chimera-shared-ake-probe", "--features", "live"],
        cwd=workspace,
        text=True,
        capture_output=True,
    )
    if build.returncode != 0:
        sys.stdout.write(build.stdout)
        sys.stderr.write(build.stderr)
        return build.returncode

    was_active = service_active()
    try:
        if was_active:
            subprocess.run(["sudo", "-n", "systemctl", "stop", SERVICE], check=True, timeout=30)
        env = dict(**__import__("os").environ)
        env["LD_LIBRARY_PATH"] = str(evdi_lib) + (f":{env['LD_LIBRARY_PATH']}" if env.get("LD_LIBRARY_PATH") else "")
        probe = subprocess.run([str(binary)], cwd=workspace, env=env, text=True, capture_output=True, timeout=45)
        sys.stdout.write(probe.stdout)
        sys.stderr.write(probe.stderr)
        return probe.returncode
    finally:
        if was_active:
            try:
                subprocess.run(["sudo", "-n", "systemctl", "start", SERVICE], check=True, timeout=30)
            finally:
                for _ in range(50):
                    if service_active():
                        break
                    time.sleep(0.2)
        print(f"service-restored={'yes' if (not was_active or service_active()) else 'no'}")


if __name__ == "__main__":
    raise SystemExit(main())
