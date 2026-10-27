#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

REPOSITORY = Path(__file__).resolve().parent.parent
SAMPLE_SECONDS = (1, 5, 15)
CONFIG = '''autostart = []
[general]
backend = "winit"
hot_reload = false
[integrations]
dbus = false
[outputs.winit-0]
scale = 1.0
[power]
idle_dim = false
'''


def sample(pid, started):
    process_dir = Path(f'/proc/{pid}')
    stat = (process_dir / 'stat').read_text().rsplit(')', 1)[1].split()
    memory = {}
    for line in (process_dir / 'smaps_rollup').read_text().splitlines():
        match = re.fullmatch(r'(\w+):\s+(\d+) kB', line)
        if match:
            memory[match[1]] = int(match[2])
    gpu = []
    for entry in (process_dir / 'fdinfo').iterdir():
        try:
            fields = [
                line for line in entry.read_text().splitlines() if line.startswith('drm-')
            ]
        except FileNotFoundError:
            continue
        if fields:
            gpu.append({'fd': entry.name, 'fields': fields})
    return {
        'elapsed_seconds': round(time.monotonic() - started, 3),
        'cpu_seconds': (int(stat[11]) + int(stat[12])) / os.sysconf('SC_CLK_TCK'),
        'memory_kib': memory,
        'drm_fdinfo': gpu,
    }


def measure(binary, root):
    config = root / 'config' / 'blair'
    config.mkdir(parents=True)
    (config / 'config.toml').write_text(CONFIG)
    environment = dict(
        os.environ,
        XDG_CONFIG_HOME=str(root / 'config'),
        XDG_STATE_HOME=str(root / 'state'),
        BLAIR_LOG_STDERR='1',
        RUST_LOG='blair=debug,warn',
    )
    log = root / 'compositor.log'
    started = time.monotonic()
    with log.open('w') as output:
        process = subprocess.Popen(
            [str(binary)], env=environment, stdout=output, stderr=subprocess.STDOUT
        )
    try:
        deadline = started + 20
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f'Blair exited with {process.returncode}; see {log}')
            if 'entering the nested event loop' in log.read_text():
                break
            time.sleep(0.05)
        else:
            raise RuntimeError(f'Blair did not enter the nested event loop; see {log}')
        ready = time.monotonic() - started
        socket = re.search(r'socket=(wayland-\d+)', log.read_text()).group(1)
        info = subprocess.run(
            ['wayland-info'],
            env=dict(environment, WAYLAND_DISPLAY=socket),
            capture_output=True,
            text=True,
            timeout=5,
            check=True,
        )
        (root / 'wayland-info.txt').write_text(info.stdout)
        samples = []
        for offset in SAMPLE_SECONDS:
            time.sleep(max(0, started + offset - time.monotonic()))
            if process.poll() is not None:
                raise RuntimeError(f'Blair exited with {process.returncode}; see {log}')
            samples.append(sample(process.pid, started))
        return {'ready_seconds': round(ready, 3), 'samples': samples}
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description='Sample an empty nested Blair session on Linux.')
    parser.add_argument(
        '--binary', type=Path, default=REPOSITORY / 'target/release/blair',
        help='Path to a release binary.',
    )
    parser.add_argument(
        '--output', type=Path,
        help='New directory for measurements, configuration, and logs.',
    )
    arguments = parser.parse_args()
    binary = arguments.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f'Build the release binary first: {binary}')
    if not shutil.which('wayland-info'):
        parser.error('wayland-info is required')
    if arguments.output:
        root = arguments.output.resolve()
        root.mkdir(parents=True)
    else:
        root = Path(tempfile.mkdtemp(prefix='blair-memory-'))
    result = {
        'commit': subprocess.check_output(
            ['git', 'rev-parse', 'HEAD'], cwd=REPOSITORY, text=True
        ).strip(),
        'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'kernel': platform.release(),
        'profile': 'release',
        'backend': 'winit',
        'shell': False,
        'dbus': False,
        'system_configuration_present': Path('/etc/blair').exists(),
    }
    result.update(measure(binary, root))
    (root / 'measurements.json').write_text(json.dumps(result, indent=2) + '\n')
    print(root / 'measurements.json')


if __name__ == '__main__':
    main()
