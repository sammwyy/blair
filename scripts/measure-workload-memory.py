#!/usr/bin/env python3
import argparse
import hashlib
import importlib.util
import json
import os
import platform
import re
import shlex
import shutil
import signal
import subprocess
import tempfile
import time
from contextlib import ExitStack
from pathlib import Path
from urllib.parse import quote

REPOSITORY = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('nested_memory', Path(__file__).with_name('measure-nested-memory.py'))
nested = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nested)
SERVICE = 'org.blair.Compositor'
OBJECT = '/org/blair/Compositor'
INTERFACE = 'org.blair.Compositor1'
CONFIG = nested.CONFIG.replace('dbus = false', 'dbus = true') + '\n[window]\nwork_area_padding = 0\n'


def executable(command):
    arguments = shlex.split(command)
    if not arguments:
        raise ValueError('the command is empty')
    path = shutil.which(arguments[0])
    if not path:
        raise ValueError(f'executable not found: {arguments[0]}')
    arguments[0] = str(Path(path).resolve())
    return arguments


def stop(process):
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)


def wait_for(predicate, processes, description, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for name, process in processes:
            if process.poll() is not None:
                raise RuntimeError(f'{name} exited with {process.returncode} while waiting for {description}')
        if predicate():
            return
        time.sleep(0.05)
    raise RuntimeError(f'timed out waiting for {description}')


def bus_call(environment, method, *arguments):
    result = subprocess.run([
        'busctl', '--address=' + environment['DBUS_SESSION_BUS_ADDRESS'], '--json=short',
        'call', SERVICE, OBJECT, INTERFACE, method, *arguments,
    ], env=environment, capture_output=True, text=True, timeout=5, check=True)
    return json.loads(result.stdout)['data'][0]


def measure(arguments, root, report):
    config = root / 'config' / 'blair'
    config.mkdir(parents=True)
    (config / 'config.toml').write_text(CONFIG)
    address = 'unix:path=' + quote(str(root / 'bus'), safe='/')
    bus_config = root / 'bus.conf'
    bus_config.write_text(f'''<busconfig>
  <type>session</type>
  <listen>{address}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow user="{os.getuid()}"/>
    <allow own="*"/>
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
  </policy>
</busconfig>
''')
    environment = dict(os.environ, XDG_CONFIG_HOME=str(root / 'config'),
        XDG_STATE_HOME=str(root / 'state'), DBUS_SESSION_BUS_ADDRESS=address,
        BLAIR_LOG_STDERR='1', RUST_LOG='blair=debug,warn')
    processes = []
    windows = []
    started = time.monotonic()
    with ExitStack() as stack:
        def start(command, name, env):
            output = stack.enter_context((root / f'{name}.log').open('w'))
            process = subprocess.Popen(command, env=env, stdout=output,
                stderr=subprocess.STDOUT, start_new_session=True)
            report['process_ids'][name] = process.pid
            stack.callback(stop, process)
            return process

        bus = start(['dbus-daemon', '--config-file=' + str(bus_config), '--nofork', '--nopidfile'], 'bus', environment)
        processes.append(('bus', bus))
        wait_for(lambda: (root / 'bus').exists(), processes, 'private D-Bus socket')
        compositor = start([str(arguments.binary)], 'compositor', environment)
        processes.append(('compositor', compositor))
        log = root / 'compositor.log'
        wait_for(lambda: 'entering the nested event loop' in log.read_text(), processes, 'nested event loop')
        report['ready_seconds'] = round(time.monotonic() - started, 3)
        socket = re.search(r'socket=(wayland-\d+)', log.read_text()).group(1)
        clients = dict(environment, WAYLAND_DISPLAY=socket, XDG_CURRENT_DESKTOP='Blair', XDG_SESSION_TYPE='wayland')
        clients.pop('DISPLAY', None)
        info = subprocess.run(['wayland-info'], env=clients, capture_output=True, text=True, timeout=5, check=True)
        (root / 'wayland-info.txt').write_text(info.stdout)
        def api_ready():
            try:
                bus_call(environment, 'ListWindows')
                return True
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                return False
        wait_for(api_ready, processes, 'Blair D-Bus API')

        def settle():
            deadline = time.monotonic() + arguments.settle
            while time.monotonic() < deadline:
                wait_for(lambda: True, processes, 'settling')
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))

        def window_count():
            return len(bus_call(environment, 'ListWindows'))

        shell = None

        def record(phase):
            settle()
            entry = {'phase': phase, 'window_count': window_count(),
                'displays': bus_call(environment, 'DisplayInfo'), 'output_scale': 1,
                'compositor': nested.sample(compositor.pid, started),
                'shell': nested.sample(shell.pid, started) if shell else None,
                'windows': [{'pid': window.pid, **nested.sample(window.pid, started)} for window in windows]}
            report['stages'].append(entry)
            (root / 'measurements.json').write_text(json.dumps(report, indent=2) + '\n')
            print(f'{phase}: windows={entry["window_count"]} compositor_rss_kib={entry["compositor"]["memory_kib"]["Rss"]}', flush=True)

        wait_for(lambda: window_count() == 0, processes, 'empty compositor')
        record('empty')
        shell = start(arguments.shell, 'shell', clients)
        processes.append(('shell', shell))
        wait_for(lambda: 'layer mapped' in log.read_text(), processes, 'mapped shell layer surface')
        record('shell-loaded')
        report['shell_mapped_layers'] = [line for line in log.read_text().splitlines() if 'layer mapped' in line]
        baseline = window_count()
        report['shell_window_count'] = baseline
        for cycle in range(1, arguments.cycles + 1):
            for index in range(arguments.windows):
                window = start(arguments.window_command, f'window-{cycle}-{index}', clients)
                windows.append(window)
            wait_for(lambda: window_count() == baseline + arguments.windows,
                processes + [('window', window) for window in windows], 'mapped application windows')
            record(f'cycle-{cycle}-open')
            configuration = bus_call(environment, 'Configuration')
            padding = 12 if cycle % 2 else 0
            configuration, replacements = re.subn(r'(?m)^work_area_padding = \d+$',
                f'work_area_padding = {padding}', configuration)
            if replacements != 1:
                raise RuntimeError('could not identify work_area_padding in compositor configuration')
            if not bus_call(environment, 'SetConfiguration', 's', configuration):
                raise RuntimeError('compositor rejected workload configuration')
            record(f'cycle-{cycle}-reconfigured')
            for window in windows:
                stop(window)
            windows.clear()
            wait_for(lambda: window_count() == baseline, processes, 'closed application windows')
            record(f'cycle-{cycle}-closed')
        report['complete'] = True


def main():
    parser = argparse.ArgumentParser(description='Measure isolated nested Blair with a shell and window/configuration churn.')
    parser.add_argument('--binary', type=Path, default=REPOSITORY / 'target/release/blair')
    parser.add_argument('--shell', required=True, help='Shell command, parsed as arguments without a shell.')
    parser.add_argument('--window-command', required=True, help='Command creating one persistent application window.')
    parser.add_argument('--windows', type=int, default=6)
    parser.add_argument('--cycles', type=int, default=2)
    parser.add_argument('--settle', type=float, default=3)
    parser.add_argument('--output', type=Path, help='New artifact directory.')
    arguments = parser.parse_args()
    if arguments.windows < 1 or arguments.cycles < 1 or arguments.settle <= 0:
        parser.error('windows, cycles, and settle must be positive')
    for command in ['dbus-daemon', 'busctl', 'wayland-info']:
        if not shutil.which(command):
            parser.error(f'{command} is required')
    try:
        arguments.binary = arguments.binary.resolve()
        if not arguments.binary.is_file() or not os.access(arguments.binary, os.X_OK):
            raise ValueError(f'executable not found: {arguments.binary}')
        arguments.shell = executable(arguments.shell)
        arguments.window_command = executable(arguments.window_command)
    except ValueError as error:
        parser.error(str(error))
    root = arguments.output.resolve() if arguments.output else Path(tempfile.mkdtemp(prefix='blair-workload-memory-'))
    if arguments.output:
        root.mkdir(parents=True)
    profile = 'release' if arguments.binary == (REPOSITORY / 'target/release/blair').resolve() else 'unverified'
    report = {'complete': False, 'backend': 'winit', 'profile': profile, 'dbus': True,
        'private_bus': True, 'service_activation': False, 'kernel': platform.release(), 'stages': [], 'process_ids': {},
        'commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPOSITORY, text=True).strip(),
        'checkout_dirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=REPOSITORY)),
        'system_configuration_present': Path('/etc/blair').exists(),
        'windows_per_cycle': arguments.windows, 'cycles': arguments.cycles, 'settle_seconds': arguments.settle,
        'commands': {'compositor': [str(arguments.binary)], 'shell': arguments.shell, 'window': arguments.window_command},
        'binary_sha256': {role: hashlib.sha256(Path(command[0]).read_bytes()).hexdigest()
            for role, command in [('compositor', [str(arguments.binary)]), ('shell', arguments.shell), ('window', arguments.window_command)]}}
    try:
        measure(arguments, root, report)
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        (root / 'measurements.json').write_text(json.dumps(report, indent=2) + '\n')
        print(root / 'measurements.json', flush=True)


if __name__ == '__main__':
    main()
