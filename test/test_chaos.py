#!/usr/bin/env python3
import os
import sys
import time
import shutil
import random
import string
import hashlib
import subprocess
import threading
from concurrent.futures import ThreadPoolExecutor

MOUNT_DIR = "/tmp/mounty-chaos-mnt"
FILER_PORT = 3001
SERVER_URL = f"http://localhost:{FILER_PORT}"

def generate_random_file(path, size_bytes):
    with open(path, 'wb') as f:
        f.write(os.urandom(size_bytes))

def hash_file(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        while chunk := f.read(8192):
            h.update(chunk)
    return h.hexdigest()

class ChaosTester:
    def __init__(self, mount_dir, num_workers=10):
        self.mount_dir = mount_dir
        self.num_workers = num_workers
        self.errors = []
        self.lock = threading.Lock()

    def report_error(self, e):
        with self.lock:
            self.errors.append(e)
            print(f"[!] Error: {e}")

    def worker_task(self, worker_id):
        try:
            # 1. Create a directory for this worker
            worker_dir = os.path.join(self.mount_dir, f"worker_{worker_id}")
            os.makedirs(worker_dir, exist_ok=True)

            # 2. Create multiple files concurrently
            files = []
            for i in range(5):
                file_path = os.path.join(worker_dir, f"file_{i}.bin")
                size = random.randint(1024, 1024 * 1024 * 5) # 1KB to 5MB
                generate_random_file(file_path, size)
                files.append((file_path, hash_file(file_path)))
            
            # 3. Read and verify files
            for file_path, original_hash in files:
                current_hash = hash_file(file_path)
                if current_hash != original_hash:
                    raise ValueError(f"Checksum mismatch for {file_path}")
            
            # 4. Rename operations
            new_files = []
            for file_path, original_hash in files:
                new_path = file_path + "_renamed"
                os.rename(file_path, new_path)
                new_files.append((new_path, original_hash))
            
            # 5. Verify after rename
            for file_path, original_hash in new_files:
                current_hash = hash_file(file_path)
                if current_hash != original_hash:
                    raise ValueError(f"Checksum mismatch after rename for {file_path}")
            
            # 6. Delete some files
            for file_path, _ in new_files[:2]:
                os.remove(file_path)
            
            # 7. Create nested directories
            nested_dir = os.path.join(worker_dir, "nested", "dirs", "test")
            os.makedirs(nested_dir, exist_ok=True)
            
            # 8. List directory
            os.listdir(worker_dir)
            
        except Exception as e:
            self.report_error(e)

    def run(self):
        print(f"[*] Starting chaos test with {self.num_workers} workers...")
        with ThreadPoolExecutor(max_workers=self.num_workers) as executor:
            futures = [executor.submit(self.worker_task, i) for i in range(self.num_workers)]
            for future in futures:
                future.result()
        
        if self.errors:
            print(f"[!] Chaos test completed with {len(self.errors)} errors.")
            return False
        else:
            print("[*] Chaos test completed successfully with 0 errors.")
            return True

def cleanup(filer_proc, mounty_proc):
    print("[*] Cleaning up...")
    if mounty_proc:
        try:
            if sys.platform == "darwin":
                subprocess.run(["umount", MOUNT_DIR], stderr=subprocess.DEVNULL)
            else:
                subprocess.run(["fusermount", "-u", MOUNT_DIR], stderr=subprocess.DEVNULL)
            mounty_proc.terminate()
            mounty_proc.wait(timeout=5)
        except Exception as e:
            print(f"[!] Failed to stop mounty: {e}")
            mounty_proc.kill()
    
    if filer_proc:
        try:
            filer_proc.terminate()
            filer_proc.wait(timeout=5)
        except Exception as e:
            print(f"[!] Failed to stop filer: {e}")
            filer_proc.kill()
    
    if os.path.exists(MOUNT_DIR):
        try:
            os.rmdir(MOUNT_DIR)
        except:
            pass
    print("[*] Cleanup complete.")

def main():
    proj_dir = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    
    print("[*] Building mounty and filer (release mode)...")
    subprocess.run(["cargo", "build", "--release", "--manifest-path", os.path.join(proj_dir, "filer", "Cargo.toml")], check=True)
    subprocess.run(["cargo", "build", "--release", "--manifest-path", os.path.join(proj_dir, "mounty", "Cargo.toml")], check=True)

    filer_bin = os.path.join(proj_dir, "filer", "target", "release", "remote-fs-server")
    mounty_bin = os.path.join(proj_dir, "mounty", "target", "release", "mounty")

    os.makedirs(MOUNT_DIR, exist_ok=True)
    
    filer_proc = None
    mounty_proc = None
    try:
        print(f"[*] Starting filer server on port {FILER_PORT}...")
        env = os.environ.copy()
        env["BASE_DIR"] = "/tmp/filer-test-base"
        os.makedirs(env["BASE_DIR"], exist_ok=True)
        filer_proc = subprocess.Popen([filer_bin, "--port", str(FILER_PORT)], env=env)
        time.sleep(2)
        
        print(f"[*] Creating mountpoint {MOUNT_DIR} and starting mounty...")
        mounty_proc = subprocess.Popen([mounty_bin, SERVER_URL, MOUNT_DIR])
        time.sleep(3)

        if filer_proc.poll() is not None:
            print("[!] Filer failed to start.")
            sys.exit(1)
            
        if mounty_proc.poll() is not None:
            print("[!] Mounty failed to start.")
            sys.exit(1)
            
        tester = ChaosTester(MOUNT_DIR, num_workers=20)
        success = tester.run()
        
        if not success:
            sys.exit(1)
            
    except Exception as e:
        print(f"[!] Unexpected error: {e}")
        sys.exit(1)
    finally:
        cleanup(filer_proc, mounty_proc)

if __name__ == "__main__":
    main()
