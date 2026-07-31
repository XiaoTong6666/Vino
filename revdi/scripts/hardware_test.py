#!/usr/bin/env python3
"""Guarded Vino hardware takeover and display validation."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
from typing import IO, NoReturn


SUPPORTED_PRODUCTS = {"6006", "433f"}
DEFAULT_SERVICE = "displaylink-driver.service"


class TestFailure(RuntimeError):
    """A hardware-test assertion or prerequisite failed."""


def run(
    command: list[str],
    *,
    check: bool = True,
    capture: bool = False,
    timeout: float | None = None,
    env: dict[str, str] | None = None,
    stdout: IO[bytes] | None = None,
) -> subprocess.CompletedProcess[str] | subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        command,
        check=check,
        text=not stdout,
        capture_output=capture,
        timeout=timeout,
        env=env,
        stdout=stdout,
        stderr=subprocess.STDOUT if stdout else None,
    )


def command_output(command: list[str], *, check: bool = True) -> str:
    result = run(command, check=check, capture=True)
    assert isinstance(result.stdout, str)
    return result.stdout


def service_active(service: str) -> bool:
    result = run(["systemctl", "is-active", "--quiet", service], check=False)
    return result.returncode == 0


def process_exists(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def service_guard(args: argparse.Namespace) -> int:
    """Privileged helper: stop one service and restore it on every exit path."""
    stopped = False

    def stop_signal(_signum: int, _frame: object) -> NoReturn:
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop_signal)
    signal.signal(signal.SIGINT, stop_signal)
    try:
        run(["systemctl", "stop", args.guard_service], timeout=30)
        stopped = True
        Path(args.guard_ready).touch()
        done = Path(args.guard_done)
        while not done.exists() and process_exists(args.guard_parent):
            time.sleep(0.2)
        return 0
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"service guard failed: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
    finally:
        if stopped:
            try:
                run(["systemctl", "start", args.guard_service], timeout=30)
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
                print(f"service restore failed: {error}", file=sys.stderr)


class HardwareTest:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.root = Path(__file__).resolve().parents[2]
        self.workspace = self.root / "revdi"
        self.evdi_dir = self.root / "evdi"
        self.binary = self.workspace / "target/release/chimera"
        stamp = time.strftime("%Y%m%d-%H%M%S")
        self.log_dir = Path(os.environ.get("VINO_HW_LOG_DIR", f"/tmp/vino-hardware-{stamp}"))
        self.log_dir.mkdir(parents=True, exist_ok=False)
        self.service = os.environ.get("DISPLAYLINK_SERVICE", DEFAULT_SERVICE)
        self.service_was_active = False
        self.privilege_mode = ""
        self.daemon: subprocess.Popen[bytes] | None = None
        self.guard: subprocess.Popen[bytes] | None = None
        self.guard_log: IO[bytes] | None = None
        self.daemon_log: IO[bytes] | None = None
        self.workload: subprocess.Popen[bytes] | None = None
        self.workload_log: IO[bytes] | None = None
        self.guard_ready = self.log_dir / "service-stopped"
        self.guard_done = self.log_dir / "test-finished"
        self.restored = False

    def execute(self) -> None:
        self.check_prerequisites()
        self.capture_baseline()
        self.build()
        self.stop_service()
        self.start_daemon()
        self.wait_for_log(r"monitor connected", 45)
        self.enable_connected_outputs()
        self.wait_for_log(r"event mode_changed [0-9]+x[0-9]+@", 45)
        self.wait_for_log(r"frame kind=keyframe", 30)
        self.write_command_log("xrandr-vino.txt", ["xrandr", "--query"])
        self.create_damage_workload()
        self.dwell(self.args.duration)
        self.assert_frame_results()
        if self.args.interactive_hotplug:
            self.test_hotplug_and_multihead()

    def check_prerequisites(self) -> None:
        if not self.args.ack_display_disruption:
            raise TestFailure(
                "refusing to disrupt the active display without --ack-display-disruption"
            )
        if self.args.duration < 15:
            raise TestFailure("--duration must be at least 15 seconds")
        if not os.environ.get("DISPLAY"):
            raise TestFailure("DISPLAY is not set; run this from the graphical login session")
        if run(["sudo", "-n", "true"], check=False).returncode == 0:
            self.privilege_mode = "sudo"
        elif shutil.which("pkexec"):
            self.privilege_mode = "pkexec"
        else:
            raise TestFailure("neither authorized sudo nor PolicyKit is available")

        usb = command_output(["lsusb", "-d", "17e9:"], check=False)
        matches = re.findall(r"ID 17e9:([0-9a-fA-F]{4})", usb)
        supported = [product.lower() for product in matches if product.lower() in SUPPORTED_PRODUCTS]
        if not supported:
            raise TestFailure("no supported DisplayLink dock is connected")
        print(f"supported DisplayLink product: 17e9:{supported[0]}")

    def capture_baseline(self) -> None:
        self.service_was_active = service_active(self.service)
        self.write_command_log(
            "service-before.txt", ["systemctl", "status", self.service, "--no-pager"], check=False
        )
        self.write_command_log("lsusb-before.txt", ["lsusb"])
        self.write_command_log("lsusb-tree-before.txt", ["lsusb", "-t"])
        self.write_command_log("xrandr-before.txt", ["xrandr", "--query"])
        self.write_command_log("lsmod-before.txt", ["lsmod"])

    def build(self) -> None:
        with (self.log_dir / "build.log").open("wb") as log:
            run(["make", "-C", str(self.workspace), "chimera"], stdout=log)

    def stop_service(self) -> None:
        if not self.service_was_active:
            return
        print(f"stopping {self.service}; connected displays may blank", flush=True)
        if self.privilege_mode == "sudo":
            run(["sudo", "-n", "systemctl", "stop", self.service], timeout=30)
        else:
            self.guard_log = (self.log_dir / "service-guard.log").open("wb")
            command = [
                "pkexec",
                sys.executable,
                str(Path(__file__).resolve()),
                "--service-guard",
                "--guard-service",
                self.service,
                "--guard-ready",
                str(self.guard_ready),
                "--guard-done",
                str(self.guard_done),
                "--guard-parent",
                str(os.getpid()),
            ]
            self.guard = subprocess.Popen(command, stdout=self.guard_log, stderr=subprocess.STDOUT)
            self.wait_for_guard()
        if service_active(self.service):
            raise TestFailure(f"failed to stop {self.service}")

    def wait_for_guard(self) -> None:
        assert self.guard is not None
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if self.guard_ready.exists():
                return
            result = self.guard.poll()
            if result is not None:
                raise TestFailure(f"PolicyKit service guard exited with status {result}")
            time.sleep(0.1)
        raise TestFailure("timed out waiting for the privileged service guard")

    def start_daemon(self) -> None:
        environment = os.environ.copy()
        environment["LD_LIBRARY_PATH"] = str(self.evdi_dir / "library") + (
            f":{environment['LD_LIBRARY_PATH']}" if environment.get("LD_LIBRARY_PATH") else ""
        )
        environment["VINO_TRACE_FRAMES"] = "1"
        self.daemon_log = (self.log_dir / "daemon.log").open("wb")
        self.daemon = subprocess.Popen(
            ["stdbuf", "-oL", "-eL", str(self.binary)],
            cwd=self.workspace,
            env=environment,
            stdout=self.daemon_log,
            stderr=subprocess.STDOUT,
        )

    def daemon_text(self) -> str:
        if self.daemon_log:
            self.daemon_log.flush()
        path = self.log_dir / "daemon.log"
        return path.read_text(errors="replace") if path.exists() else ""

    def wait_for_log(self, pattern: str, timeout: float) -> None:
        assert self.daemon is not None
        regex = re.compile(pattern)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            text = self.daemon_text()
            if regex.search(text):
                return
            failures = re.findall(r"chimera: session stopped: (.+); retrying", text)
            if len(failures) >= 3:
                raise TestFailure(f"Vino session repeatedly failed: {failures[-1]}")
            result = self.daemon.poll()
            if result is not None:
                raise TestFailure(
                    f"Vino daemon exited with status {result} before matching {pattern!r}"
                )
            time.sleep(0.5)
        raise TestFailure(f"timed out waiting for daemon log {pattern!r}")

    def dwell(self, seconds: int) -> None:
        assert self.daemon is not None
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            result = self.daemon.poll()
            if result is not None:
                raise TestFailure(f"Vino daemon exited unexpectedly with status {result}")
            time.sleep(min(0.5, deadline - time.monotonic()))

    def enable_connected_outputs(self) -> None:
        if shutil.which("kscreen-doctor"):
            output = command_output(["kscreen-doctor", "-o"], check=False)
            output = re.sub(r"\x1b\[[0-9;]*m", "", output)
            blocks = re.split(r"(?=Output:\s*\d+)", output)
            enabled = []
            for block in blocks:
                match = re.match(r"Output:\s*(\d+)", block)
                if (
                    not match
                    or "DVI-I-" not in block
                    or "connected" not in block
                    or "disabled" not in block
                ):
                    continue
                output_id = match.group(1)
                run(["kscreen-doctor", f"output.{output_id}.enable"], timeout=15)
                enabled.append(output_id)
            if enabled:
                print(f"enabled KScreen outputs: {', '.join(enabled)}")
            return
        run(["xrandr", "--auto"], check=False, timeout=15)

    def create_damage_workload(self) -> None:
        if self.args.pattern_client:
            pattern = Path(self.args.pattern_client).resolve()
            if not pattern.is_file():
                raise TestFailure(f"pattern client not found: {pattern}")
            env = os.environ.copy()
            env.update(
                {
                    "DL_PATTERN_OUTPUT": "DVI-I-1",
                    "DL_PATTERN_RECT_W": "64",
                    "DL_PATTERN_RECT_H": "64",
                    "DL_PATTERN_RECT_X": "512",
                    "DL_PATTERN_RECT_Y": "384",
                    "DL_PATTERN_MODE": "rectangle",
                    "DL_PATTERN_AUTO_TOGGLE_MS": "1000",
                }
            )
            self.workload_log = (self.log_dir / "pattern-client.log").open("wb")
            self.workload = subprocess.Popen(
                [str(pattern)],
                env=env,
                stdout=self.workload_log,
                stderr=subprocess.STDOUT,
            )
            time.sleep(2)
            if self.workload.poll() is not None:
                raise TestFailure(f"pattern client exited with {self.workload.returncode}")
            return
        output = command_output(["xrandr", "--query"])
        active: list[tuple[int, int, str]] = []
        geometry = re.compile(r"\b\d+x\d+\+(\d+)\+(\d+)\b")
        for line in output.splitlines():
            if " connected" not in line:
                continue
            match = geometry.search(line)
            if match:
                active.append((int(match.group(1)), int(match.group(2)), line.split()[0]))
        if not active:
            raise TestFailure("XRandR reported no active output for the damage workload")
        output_x, output_y, output_name = max(active)
        (self.log_dir / "summary.txt").write_text(f"damage workload output: {output_name}\n")
        print(f"damage workload output: {output_name}")
        for index in range(1, 4):
            run(
                [
                    "xmessage",
                    "-geometry",
                    f"160x100+{output_x + 64 + index * 16}+{output_y + 64}",
                    "-buttons",
                    "OK:0",
                    "-timeout",
                    "2",
                    f"Vino damage test {index}",
                ],
                check=False,
                timeout=5,
            )

    def assert_frame_results(self) -> None:
        text = self.daemon_text()
        keyframes = re.findall(r"frame kind=keyframe.*encoded_bytes=(\d+)", text)
        damage_frames = re.findall(r"frame kind=damage.*encoded_bytes=(\d+)", text)
        heads = set(re.findall(r"head (\d+): (?:EVDI )?monitor connected", text))
        transport_stops = re.findall(r"session stopped:.*", text)
        if transport_stops:
            raise TestFailure(f"transport session stopped: {transport_stops[-1]}")
        if not keyframes:
            raise TestFailure("no keyframe was submitted")
        if not damage_frames:
            raise TestFailure("no incremental damage frame was submitted")
        keyframe_bytes = max(map(int, keyframes))
        damage_bytes = min(map(int, damage_frames))
        if damage_bytes <= 0:
            raise TestFailure(f"damage payload was empty ({damage_bytes})")
        if self.args.pattern_client:
            pattern_frames = re.findall(
                r"frame kind=damage .*effective_strips=(\d+) encoded_bytes=(\d+) "
                r"damage_bounds=Some\(\(512, 384, 576, 448\)\)",
                text,
            )
            if not any(int(strips) == 4 and int(size) < keyframe_bytes for strips, size in pattern_frames):
                raise TestFailure(
                    "no four-strip 64x64 pattern update smaller than the keyframe was submitted"
                )
        print(
            "PASS: "
            f"keyframes={len(keyframes)} damage_frames={len(damage_frames)} "
            f"keyframe_bytes={keyframe_bytes} damage_bytes={damage_bytes} "
            f"connected_heads={len(heads)}"
        )

    def test_hotplug_and_multihead(self) -> None:
        input("Unplug the active DisplayLink monitor, then press Enter: ")
        self.wait_for_log(r"monitor disconnected", 30)
        connections = self.daemon_text().count("monitor connected")
        keyframes = self.daemon_text().count("frame kind=keyframe")
        input("Reconnect the monitor, then press Enter: ")
        self.wait_for_count("monitor connected", connections + 1, 30)
        self.wait_for_count("frame kind=keyframe", keyframes + 1, 30)
        input("Connect a second monitor if available, then press Enter (or Enter to skip): ")
        self.dwell(10)
        heads = set(re.findall(r"head (\d+): monitor connected", self.daemon_text()))
        if len(heads) >= 2:
            print(f"PASS: multi-head discovery observed {len(heads)} heads")
        else:
            print("SKIP: second physical monitor was not observed")

    def wait_for_count(self, marker: str, expected: int, timeout: float) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.daemon_text().count(marker) >= expected:
                return
            if self.daemon and self.daemon.poll() is not None:
                raise TestFailure("Vino daemon exited during hotplug validation")
            time.sleep(0.5)
        raise TestFailure(f"timed out waiting for {marker!r} count {expected}")

    def write_command_log(self, name: str, command: list[str], *, check: bool = True) -> None:
        with (self.log_dir / name).open("wb") as log:
            run(command, check=check, stdout=log)

    def restore(self) -> None:
        if self.restored:
            return
        self.restored = True
        if self.workload and self.workload.poll() is None:
            self.workload.terminate()
            try:
                self.workload.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.workload.kill()
                self.workload.wait(timeout=2)
        if self.workload_log:
            self.workload_log.close()
        if self.daemon and self.daemon.poll() is None:
            self.daemon.terminate()
            try:
                self.daemon.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.daemon.kill()
                try:
                    self.daemon.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    print(
                        "warning: Vino daemon remained blocked after SIGKILL; continuing restore",
                        file=sys.stderr,
                    )
        if self.daemon_log:
            self.daemon_log.close()

        if self.service_was_active:
            if self.guard:
                self.guard_done.touch()
                try:
                    self.guard.wait(timeout=40)
                except subprocess.TimeoutExpired:
                    print("warning: service guard did not exit after restore request", file=sys.stderr)
            elif not service_active(self.service):
                try:
                    run([*self.privileged_command(), "systemctl", "start", self.service], timeout=30)
                except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
                    print(f"warning: direct service restore failed: {error}", file=sys.stderr)
        if self.guard_log:
            self.guard_log.close()
        for _ in range(50):
            if not self.service_was_active or service_active(self.service):
                break
            time.sleep(0.2)
        self.write_command_log(
            "service-after.txt", ["systemctl", "status", self.service, "--no-pager"], check=False
        )
        self.write_command_log("xrandr-after.txt", ["xrandr", "--query"], check=False)
        print(f"logs: {self.log_dir}")

    def privileged_command(self) -> list[str]:
        return ["sudo", "-n"] if self.privilege_mode == "sudo" else ["pkexec"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ack-display-disruption", action="store_true")
    parser.add_argument("--duration", type=int, default=60)
    parser.add_argument("--pattern-client")
    parser.add_argument("--interactive-hotplug", action="store_true")
    parser.add_argument("--service-guard", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--guard-service", help=argparse.SUPPRESS)
    parser.add_argument("--guard-ready", help=argparse.SUPPRESS)
    parser.add_argument("--guard-done", help=argparse.SUPPRESS)
    parser.add_argument("--guard-parent", type=int, help=argparse.SUPPRESS)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.service_guard:
        required = (args.guard_service, args.guard_ready, args.guard_done, args.guard_parent)
        if any(value is None for value in required):
            print("incomplete service-guard arguments", file=sys.stderr)
            return 2
        return service_guard(args)

    def stop_signal(_signum: int, _frame: object) -> NoReturn:
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, stop_signal)
    signal.signal(signal.SIGINT, stop_signal)

    test: HardwareTest | None = None
    try:
        test = HardwareTest(args)
        test.execute()
        return 0
    except (TestFailure, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("hardware test interrupted", file=sys.stderr)
        return 130
    finally:
        if test:
            test.restore()


if __name__ == "__main__":
    raise SystemExit(main())
