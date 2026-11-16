#!/usr/bin/env python3
"""Exercise the packaged executable through its launcher against a running headless IPC peer."""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading


def read_line(process, timeout=15):
    result = queue.Queue()
    threading.Thread(target=lambda: result.put(process.stdout.readline()), daemon=True).start()
    line = result.get(timeout=timeout)
    assert line, "fixture exited before reporting a request"
    return line


def main():
    app, helper = map(Path, sys.argv[1:])
    launcher = app / "Contents/SharedSupport/bin/velocity"
    subprocess.run([str(launcher), "--help"], stdout=subprocess.DEVNULL, check=True, timeout=10)
    with tempfile.TemporaryDirectory(prefix="velocity-cli-") as temporary:
        root = Path(temporary).resolve()
        profile, closed = root / "fresh profile", root / "sheet.closed"
        fixture = subprocess.Popen([str(helper), str(profile), str(os.getuid()), str(closed)],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        client = None
        socket_path = None
        try:
            socket_path = Path(read_line(fixture).strip())
            bin_dir = root / "bin with spaces"
            bin_dir.mkdir()
            link = bin_dir / "velocity"
            link.symlink_to(launcher)
            first = subprocess.run([str(link), "--data-dir", str(profile), "-n", "--project", "project with spaces.sublime-project",
                                    "hello 世界.rs:12:3", "--", "-source"], cwd=root,
                                   capture_output=True, timeout=10)
            assert first.returncode == 0, first.stderr
            request = json.loads(read_line(fixture))
            assert request["new_window"] and not request["add"] and not request["wait"]
            assert request["project"] == str(root / "project with spaces.sublime-project")
            assert request["files"] == [
                {"path": str(root / "hello 世界.rs"), "line": 12, "column": 3},
                {"path": str(root / "-source"), "line": 1, "column": 1}]
            client = subprocess.Popen([str(link), "--data-dir", str(profile), "-a", "-w", "-"], cwd=root,
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            client.stdin.write("stdin payload 你好\n")
            client.stdin.close()
            request = json.loads(read_line(fixture))
            assert request["add"] and request["wait"] and request["stdin"] == "stdin payload 你好\n"
            assert request["files"] == []
            try:
                client.wait(timeout=0.2)
                raise AssertionError("--wait returned before the peer closed its sheet")
            except subprocess.TimeoutExpired:
                pass
            closed.touch()
            assert client.wait(timeout=10) == 0
            assert fixture.wait(timeout=10) == 0, fixture.stderr.read()
            assert not profile.exists(), "forwarding created an unnecessary profile"
            print("CLI forwarding passed: paths, Unicode locations, flags, symlink, stdin, close-driven --wait")
        finally:
            if client and client.poll() is None:
                client.kill()
                client.wait()
            if fixture.poll() is None:
                fixture.kill()
                fixture.wait()
            if socket_path:
                socket_path.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
