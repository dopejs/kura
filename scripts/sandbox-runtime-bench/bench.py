"""Workloads an agent sandbox run does, timed inside the container so
container start-up is measured separately. Prints one JSON object (ms)."""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time


def timed(fn):
    t = time.perf_counter()
    fn()
    return round((time.perf_counter() - t) * 1000, 1)


def spawn():
    for _ in range(300):
        subprocess.run(["/bin/true"], check=True)


def cpu():
    h = hashlib.sha256()
    chunk = b"\0" * (1 << 20)
    for _ in range(256):
        h.update(chunk)


def small_files(root):
    def run():
        d = os.path.join(root, "bench-small")
        os.makedirs(d, exist_ok=True)
        data = b"x" * 1024
        for i in range(2000):
            with open(os.path.join(d, f"f{i}"), "wb") as f:
                f.write(data)
        for i in range(2000):
            with open(os.path.join(d, f"f{i}"), "rb") as f:
                f.read()
        shutil.rmtree(d)
    return run


def big_file(root):
    def run():
        p = os.path.join(root, "bench-big")
        block = b"y" * (1 << 20)
        with open(p, "wb") as f:
            for _ in range(200):
                f.write(block)
            f.flush()
            os.fsync(f.fileno())
        with open(p, "rb") as f:
            while f.read(1 << 20):
                pass
        os.remove(p)
    return run


def stats(root):
    def run():
        p = os.path.join(root, "bench-stat")
        open(p, "w").close()
        for _ in range(100_000):
            os.stat(p)
        os.remove(p)
    return run


def imports():
    for _ in range(10):
        subprocess.run([sys.executable, "-c", "import json, email, http.client, asyncio, sqlite3, ssl"], check=True)


ws = "/workspace"
print(json.dumps({
    "spawn_300": timed(spawn),
    "sha256_256MiB": timed(cpu),
    "small_files_ws_2000": timed(small_files(ws)),
    "small_files_tmp_2000": timed(small_files("/tmp")),
    "big_file_ws_200MiB": timed(big_file(ws)),
    "stat_ws_100k": timed(stats(ws)),
    "python_imports_10": timed(imports),
}))
