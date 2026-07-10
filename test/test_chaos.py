#!/usr/bin/env python3
import argparse
import hashlib
import http.client
import os
import random
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def generate_random_file(path, size_bytes):
    data = os.urandom(size_bytes)
    with open(path, "wb") as f:
        f.write(data)
    return data


def hash_bytes(data):
    return hashlib.sha256(data).hexdigest()


def hash_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(8192):
            h.update(chunk)
    return h.hexdigest()


def wait_for_http(port, timeout=10):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
            conn.request("GET", "/list")
            resp = conn.getresponse()
            resp.read()
            conn.close()
            if resp.status == 200:
                return True
        except OSError:
            pass
        time.sleep(0.1)
    return False


def is_mount_active(mount_dir):
    try:
        output = subprocess.check_output(["mount"], text=True)
    except subprocess.SubprocessError:
        return False
    canonical = os.path.realpath(mount_dir)
    return mount_dir in output or canonical in output


def wait_for_mount(mount_dir, timeout=15):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if is_mount_active(mount_dir):
            return True
        time.sleep(0.1)
    return False


def unmount(mount_dir):
    commands = (
        [["umount", mount_dir], ["diskutil", "unmount", mount_dir]]
        if sys.platform == "darwin"
        else [["fusermount3", "-u", mount_dir], ["fusermount", "-u", mount_dir], ["umount", mount_dir]]
    )
    for cmd in commands:
        try:
            if subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                return True
        except FileNotFoundError:
            continue
    return False


class ChaosTester:
    def __init__(self, mount_dir, num_workers, files_per_worker, max_file_kb):
        self.mount_dir = mount_dir
        self.num_workers = num_workers
        self.files_per_worker = files_per_worker
        self.max_file_kb = max_file_kb
        self.errors = []
        self.lock = threading.Lock()

    def report_error(self, err):
        with self.lock:
            self.errors.append(err)
            print(f"[!] Error: {err}")

    def worker_task(self, worker_id):
        try:
            worker_dir = os.path.join(self.mount_dir, f"worker_{worker_id}")
            os.makedirs(worker_dir, exist_ok=True)

            read_files = []
            for i in range(self.files_per_worker):
                file_path = os.path.join(worker_dir, f"read_file_{i}.bin")
                size = random.randint(1024, max(1024, self.max_file_kb * 1024))
                data = generate_random_file(file_path, size)
                read_files.append((file_path, hash_bytes(data)))

            for file_path, original_hash in read_files:
                current_hash = hash_file(file_path)
                if current_hash != original_hash:
                    raise ValueError(f"Checksum mismatch for {file_path}")

            rename_files = []
            for i in range(self.files_per_worker):
                file_path = os.path.join(worker_dir, f"rename_file_{i}.bin")
                size = random.randint(1024, max(1024, self.max_file_kb * 1024))
                data = generate_random_file(file_path, size)
                rename_files.append((file_path, hash_bytes(data)))

            renamed_files = []
            for file_path, original_hash in rename_files:
                new_path = file_path + "_renamed"
                os.rename(file_path, new_path)
                renamed_files.append((new_path, original_hash))

            for file_path, original_hash in renamed_files:
                current_hash = hash_file(file_path)
                if current_hash != original_hash:
                    raise ValueError(f"Checksum mismatch after rename for {file_path}")

            for file_path, _ in renamed_files[: max(1, len(renamed_files) // 2)]:
                os.remove(file_path)

            nested_dir = os.path.join(worker_dir, "nested", "dirs", "test")
            os.makedirs(nested_dir, exist_ok=True)
            os.listdir(worker_dir)
        except Exception as err:
            self.report_error(err)

    def run(self):
        print(
            f"[*] Starting chaos test with {self.num_workers} workers, "
            f"{self.files_per_worker} files/worker, max {self.max_file_kb}KB/file..."
        )
        with ThreadPoolExecutor(max_workers=self.num_workers) as executor:
            futures = [executor.submit(self.worker_task, i) for i in range(self.num_workers)]
            for future in futures:
                future.result()

        if self.errors:
            print(f"[!] Chaos test completed with {len(self.errors)} errors.")
            return False

        print("[*] Chaos test completed successfully with 0 errors.")
        return True


def cleanup(filer_proc, mounty_proc, mount_dir, base_dir):
    print("[*] Cleaning up...")
    if mounty_proc:
        unmount(mount_dir)
        try:
            mounty_proc.terminate()
            mounty_proc.wait(timeout=5)
        except Exception as err:
            print(f"[!] Failed to stop mounty gracefully: {err}")
            mounty_proc.kill()
            mounty_proc.wait(timeout=5)

    if filer_proc:
        try:
            filer_proc.terminate()
            filer_proc.wait(timeout=5)
        except Exception as err:
            print(f"[!] Failed to stop filer gracefully: {err}")
            filer_proc.kill()
            filer_proc.wait(timeout=5)

    shutil.rmtree(mount_dir, ignore_errors=True)
    shutil.rmtree(base_dir, ignore_errors=True)
    print("[*] Cleanup complete.")


def parse_args():
    parser = argparse.ArgumentParser(description="Run a FUSE chaos test against filer + mounty.")
    parser.add_argument("--workers", type=int, default=20)
    parser.add_argument("--files-per-worker", type=int, default=5)
    parser.add_argument("--max-file-kb", type=int, default=5 * 1024)
    parser.add_argument("--skip-build", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    proj_dir = Path(__file__).resolve().parent.parent
    port = free_port()
    server_url = f"http://127.0.0.1:{port}"
    base_dir = tempfile.mkdtemp(prefix="filer-chaos-base-")
    mount_dir = tempfile.mkdtemp(prefix="mounty-chaos-mnt-")

    if not args.skip_build:
        print("[*] Building mounty and filer (release mode)...")
        subprocess.run(["cargo", "build", "--release", "--manifest-path", str(proj_dir / "filer" / "Cargo.toml")], check=True)
        subprocess.run(["cargo", "build", "--release", "--manifest-path", str(proj_dir / "mounty" / "Cargo.toml")], check=True)
    else:
        print("[*] Skipping build step (--skip-build).")

    filer_bin = proj_dir / "filer" / "target" / "release" / "remote-fs-server"
    mounty_bin = proj_dir / "mounty" / "target" / "release" / "mounty"

    filer_proc = None
    mounty_proc = None
    try:
        print(f"[*] Starting filer server on port {port}...")
        env = os.environ.copy()
        env["BASE_DIR"] = base_dir
        filer_proc = subprocess.Popen([str(filer_bin), "--port", str(port)], env=env)
        if not wait_for_http(port):
            raise RuntimeError("filer did not become ready")

        print(f"[*] Starting mounty at {mount_dir}...")
        mounty_proc = subprocess.Popen([str(mounty_bin), server_url, mount_dir])
        if not wait_for_mount(mount_dir):
            raise RuntimeError("mounty did not mount")

        tester = ChaosTester(
            mount_dir,
            num_workers=args.workers,
            files_per_worker=args.files_per_worker,
            max_file_kb=args.max_file_kb,
        )
        if not tester.run():
            sys.exit(1)
    except Exception as err:
        print(f"[!] Unexpected error: {err}")
        sys.exit(1)
    finally:
        cleanup(filer_proc, mounty_proc, mount_dir, base_dir)


if __name__ == "__main__":
    main()
