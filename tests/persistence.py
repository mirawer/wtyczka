#!/usr/bin/env python3
"""Persistence integration on a disposable D-Bus/keyring, never the desktop store.
Requires dbus-daemon and gnome-keyring-daemon; only synthetic keys/images/local HTTP.
All temporary files and sockets stay under the checkout's .cache/.
"""
import http.server
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import threading
import time

from smoke import API, BINARY, FAKE_IMAGE, ROOT, wait_for

KEY = 'test-secret-never-log'


def wizard(args, env, *, cancel=False, before_key=None, success=True):
    pid, master = pty.fork()
    if pid == 0:
        os.execve(str(BINARY), args, env)
    transcript = b''
    sent = False
    reaped = False
    deadline = time.monotonic() + 75
    try:
        while time.monotonic() < deadline:
            if not select.select([master], [], [], 0.2)[0]:
                continue
            try:
                chunk = os.read(master, 4096)
            except OSError:
                break
            if not chunk:
                break
            transcript += chunk
            if not sent and b'Klucz API (ukryty):' in transcript:
                import termios
                assert termios.tcgetattr(master)[3] & termios.ECHO == 0
                if before_key:
                    before_key()
                os.write(master, b'\x04' if cancel else KEY.encode() + b'\r')
                sent = True
        else:
            raise AssertionError('Setup timed out on isolated keyring')
        _, status = os.waitpid(pid, 0)
        reaped = True
        code = os.waitstatus_to_exitcode(status)
        assert sent, transcript.decode(errors='replace')
        assert (code == 0) == success, transcript.decode(errors='replace')
        assert KEY.encode() not in transcript
        assert (b'Dzia' in transcript and b'w tle' in transcript) if success else True
        return transcript
    finally:
        os.close(master)
        if not reaped:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)


def main():
    (ROOT / '.cache').mkdir(exist_ok=True)
    harness_env = os.environ.get('SCREEN_ANSWER_TEST_HARNESS')
    if harness_env:
        harness = Path(harness_env).resolve()
    else:
        candidates = [p for p in (ROOT / 'target/debug/deps').glob('screen_answer-*')
                      if p.is_file() and os.access(p, os.X_OK) and not p.suffix]
        assert candidates, 'Run cargo test first to build the isolated watcher helper'
        harness = None
        for candidate in sorted(candidates, key=lambda p: p.stat().st_mtime, reverse=True):
            listed = subprocess.run([str(candidate), '--list'], capture_output=True, timeout=10)
            if listed.returncode == 0 and b'config::tests::isolated_test_watcher: test' in listed.stdout:
                harness = candidate
                break
        assert harness, 'Run cargo test first to compile the v0.1.5 watcher helper'
    daemon = shutil.which('gnome-keyring-daemon')
    assert daemon, 'gnome-keyring-daemon required for disposable keyring test'
    with tempfile.TemporaryDirectory(prefix='persistence-', dir=ROOT / '.cache') as tmp:
        root = Path(tmp)
        for name in ['home', 'data', 'config', 'run', 'control', 'fakebin']:
            (root / name).mkdir(mode=0o700)
        address = f'unix:path={root}/bus.sock'
        env = dict(os.environ, HOME=str(root / 'home'), XDG_DATA_HOME=str(root / 'data'),
                   XDG_CONFIG_HOME=str(root / 'config'), XDG_RUNTIME_DIR=str(root / 'run'),
                   DBUS_SESSION_BUS_ADDRESS=address, GNOME_KEYRING_CONTROL=str(root / 'control'),
                   SCREEN_ANSWER_PRIVATE_TEST_DIR=str(root),
                   SCREEN_ANSWER_ISOLATED_SECRET_SERVICE_TEST='1')
        owned = []
        log = (root / 'services.log').open('wb')
        watcher = None
        server = None
        runtime = root / 'state'
        base = root / 'base.kdl'
        base.write_text('// disposable config, never loaded into the desktop\n')
        loaded = root / 'loaded'
        captures = root / 'captures'
        fakebin = root / 'fakebin'
        (fakebin / 'niri').write_text('#!/usr/bin/env python3\nimport sys,os,json\nfrom pathlib import Path\na=sys.argv[1:]\nif a[:3]==["msg","-j","focused-output"]: print(json.dumps({"name":"FAKE-1"}))\nelif a[:3]==["msg","action","load-config-file"]: Path(os.environ["TEST_LOADED"]).write_text(a[-1])\n')
        (fakebin / 'grim').write_text('#!/usr/bin/env python3\nimport sys,os\nfrom pathlib import Path\nwith Path(os.environ["TEST_CAPTURES"]).open("a") as f: f.write("capture\\n")\nsys.stdout.buffer.write(' + repr(FAKE_IMAGE) + ')\n')
        for p in fakebin.iterdir():
            p.chmod(0o700)
        env.update(PATH=f'{fakebin}:{os.environ["PATH"]}', TEST_LOADED=str(loaded), TEST_CAPTURES=str(captures))
        args = [str(BINARY), '--runtime-dir', str(runtime), '--niri-config', str(base)]

        def cli(extra=(), ok=True, selected_args=None):
            result = subprocess.run((selected_args or args) + list(extra), env=env, stdin=subprocess.DEVNULL,
                                    capture_output=True, timeout=65)
            assert (result.returncode == 0) == ok, result.stderr.decode(errors='replace')
            assert KEY.encode() not in result.stdout + result.stderr
            return result

        def bus(*extra):
            return subprocess.run(['busctl', '--address', address, '--json=short', *extra],
                                  env=env, capture_output=True, text=True, timeout=4)

        def keyring_ready():
            result = bus('get-property', 'org.freedesktop.secrets',
                         '/org/freedesktop/secrets/aliases/default', 'org.freedesktop.Secret.Collection', 'Locked')
            return result.returncode == 0 and json.loads(result.stdout)['data'] is False

        def count_credentials():
            result = bus('call', 'org.freedesktop.secrets', '/org/freedesktop/secrets',
                         'org.freedesktop.Secret.Service', 'SearchItems', 'a{ss}', '1', 'application', 'screen-answer')
            assert result.returncode == 0, result.stderr
            data = json.loads(result.stdout)['data']
            return len(data[0]) + len(data[1])

        def settings():
            return json.loads((runtime / 'settings.json').read_text())

        def worker_pid():
            with socket.socket(socket.AF_UNIX) as control:
                control.connect(str(runtime / 'control.sock'))
                return struct.unpack('3i', control.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))[0]

        def assert_old_worker_and_config(original):
            cli(['status'])
            assert (runtime / 'settings.json').read_bytes() == original

        def assert_private_files():
            assert KEY.encode() not in (runtime / 'settings.json').read_bytes()
            assert KEY.encode() not in (runtime / 'daemon.log').read_bytes()
            pid = worker_pid()
            assert KEY.encode() not in Path(f'/proc/{pid}/cmdline').read_bytes()
            assert KEY.encode() not in Path(f'/proc/{pid}/environ').read_bytes()
            assert (runtime.stat().st_mode & 0o777) == 0o700
            assert ((runtime / 'settings.json').stat().st_mode & 0o777) == 0o600

        try:
            dbus = subprocess.Popen(['dbus-daemon', '--session', '--nofork', '--nopidfile', '--address', address],
                                    env=env, stdout=log, stderr=log)
            owned.append(dbus)
            wait_for(lambda: (root / 'bus.sock').exists(), 10)
            keyring = subprocess.Popen([daemon, '--foreground', '--components=secrets', '--control-directory',
                                        str(root / 'control'), '--unlock'], env=env, stdin=subprocess.PIPE,
                                       stdout=log, stderr=log)
            owned.append(keyring)
            keyring.stdin.write(b'disposable-test-password\n')
            keyring.stdin.close()
            wait_for(keyring_ready, 15)
            # Also exercise the backend's full 4096-byte encrypted roundtrip.
            subprocess.run([str(harness), '--exact', 'credentials::linux::tests::isolated_encrypted_roundtrip_and_cleanup',
                            '--ignored', '--nocapture'], env=env, check=True, timeout=40)
            assert count_credentials() == 0
            watcher = subprocess.Popen([str(harness), '--exact', 'config::tests::isolated_test_watcher', '--ignored', '--nocapture'],
                                       env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
            owned.append(watcher)
            pending = b''
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if select.select([watcher.stdout], [], [], 0.2)[0]:
                    line = watcher.stdout.readline()
                    pending += line
                    if b'TEST_WATCHER_READY' in pending:
                        break
                    assert line, 'Isolated tray host exited'
            else:
                raise AssertionError('Isolated tray host timed out')
            server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), API)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            API.release.set()
            API.output = '2'
            flags = ['--provider', 'anthropic', '--model', 'test-model', '--base-url',
                     f'http://127.0.0.1:{server.server_port}/v1', '--keybind', 'Print',
                     '--pause-keybind', 'Ctrl+Shift+F9', '--selection', 'single', '--mode', 'digits', '--timeout', '20']
            wizard(args + flags, env)
            original = (runtime / 'settings.json').read_bytes()
            original_id = settings()['credential_id']
            assert len(original_id) == 64 and settings()['timeout'] == 20
            assert count_credentials() == 1 and API.count == 0
            assert_private_files()
            cli(['stop'])
            quick = cli()
            assert b'Konfiguracja' not in quick.stdout and b'Klucz API' not in quick.stdout
            assert (runtime / 'settings.json').read_bytes() == original
            assert_private_files()
            API.received.clear()
            cli(['trigger'])
            assert API.received.wait(5)
            assert API.models[-1] == 'test-model'
            assert API.count == 1
            # Restarting a running worker with unchanged shortcuts must not
            # treat its own bindings as external conflicts or ask for the key.
            restarted = cli(['--timeout', '90'])
            assert b'Klucz API' not in restarted.stdout
            assert settings()['timeout'] == 90 and settings()['credential_id'] == original_id
            original = (runtime / 'settings.json').read_bytes()
            # The current worker stays alive throughout a cancelled --change.
            pid = worker_pid()
            wizard(args + ['--change'] + flags, env, cancel=True, success=False,
                   before_key=lambda: assert_old_worker_and_config(original))
            assert worker_pid() == pid and (runtime / 'settings.json').read_bytes() == original
            assert count_credentials() == 1
            cli(['stop'])
            # Locked stores fail closed, without destroying settings or writing plaintext.
            alias = bus('call', 'org.freedesktop.secrets', '/org/freedesktop/secrets',
                        'org.freedesktop.Secret.Service', 'ReadAlias', 's', 'default')
            assert alias.returncode == 0
            collection = json.loads(alias.stdout)['data'][0]
            locked = bus('call', 'org.freedesktop.secrets', '/org/freedesktop/secrets',
                         'org.freedesktop.Secret.Service', 'Lock', 'ao', '1', collection)
            assert locked.returncode == 0
            assert not keyring_ready()
            unavailable = cli(ok=False)
            assert b'locked' in unavailable.stderr.lower()
            assert (runtime / 'settings.json').read_bytes() == original
            wizard(args + ['--change'] + flags, env, success=False)
            assert (runtime / 'settings.json').read_bytes() == original
            assert count_credentials() == 1
            # Restart the disposable provider to unlock without any graphical prompt.
            keyring.terminate()
            keyring.wait(timeout=5)
            keyring = subprocess.Popen([daemon, '--foreground', '--components=secrets', '--control-directory',
                                        str(root / 'control'), '--unlock'], env=env, stdin=subprocess.PIPE,
                                       stdout=log, stderr=log)
            owned.append(keyring)
            keyring.stdin.write(b'disposable-test-password\n')
            keyring.stdin.close()
            wait_for(keyring_ready, 15)
            assert count_credentials() == 1
            # Copied configuration and edited destinations cannot retrieve a key.
            copied = root / 'copied'
            copied.mkdir(mode=0o700)
            (copied / 'settings.json').write_bytes(original)
            copy_args = [str(BINARY), '--runtime-dir', str(copied), '--niri-config', str(base)]
            assert b'--change' in cli(ok=False, selected_args=copy_args).stderr
            altered = settings()
            altered['base_url'] = 'http://127.0.0.1:1/v1'
            (runtime / 'settings.json').write_text(json.dumps(altered))
            assert b'--change' in cli(ok=False).stderr
            (runtime / 'settings.json').write_bytes(original)
            assert API.count == 1 and count_credentials() == 1
            # Failed config replacement after native save must remove the pending key.
            (runtime / 'settings.json').unlink()
            (runtime / 'settings.json').mkdir()
            wizard(args + ['--change'] + flags, env, success=False)
            assert count_credentials() == 1
            assert not (runtime / 'control.sock').exists()
            (runtime / 'settings.json').rmdir()
            (runtime / 'settings.json').write_bytes(original)
            cli()
            changed_flags = flags.copy()
            changed_flags[changed_flags.index('test-model')] = 'test-model-two'
            wizard(args + ['--change'] + changed_flags, env)
            assert settings()['model'] == 'test-model-two'
            assert settings()['credential_id'] != original_id
            wait_for(lambda: count_credentials() == 1)
            assert_private_files()
            cli(['forget-key'])
            assert count_credentials() == 0 and 'credential_id' not in settings()
            assert settings()['model'] == 'test-model-two'
            assert not (runtime / 'control.sock').exists()
            cli(['status'], ok=False)
            wizard(args + changed_flags, env)
            assert count_credentials() == 1
            cli(['stop'])
            # Foreground mode must release its launcher guard after readiness,
            # so another process can forget the key and stop it normally.
            foreground_log = (root / 'foreground.log').open('wb')
            foreground = subprocess.Popen(args + ['--foreground'], env=env, stdin=subprocess.DEVNULL,
                                          stdout=foreground_log, stderr=foreground_log)
            owned.append(foreground)
            wait_for(lambda: subprocess.run(args + ['status'], env=env, capture_output=True, timeout=15).returncode == 0)
            cli(['forget-key'])
            assert foreground.wait(timeout=10) == 0
            foreground_log.close()
            assert KEY.encode() not in (root / 'foreground.log').read_bytes()
            assert count_credentials() == 0 and 'credential_id' not in settings()
            wizard(args + changed_flags, env)
            assert count_credentials() == 1
            persisted = (runtime / 'settings.json').read_bytes()
            locked = bus('call', 'org.freedesktop.secrets', '/org/freedesktop/secrets',
                         'org.freedesktop.Secret.Service', 'Lock', 'ao', '1', collection)
            assert locked.returncode == 0 and not keyring_ready()
            temporary_flags = changed_flags.copy()
            temporary_flags[temporary_flags.index('test-model-two')] = 'temporary-model'
            temporary_flags[temporary_flags.index('Print')] = 'Ctrl+Shift+F10'
            temporary_flags[temporary_flags.index('Ctrl+Shift+F9')] = 'Ctrl+Shift+F11'
            wizard(args + ['--session-only'] + temporary_flags, env)
            assert count_credentials() == 1
            assert (runtime / 'settings.json').read_bytes() == persisted
            with socket.socket(socket.AF_UNIX) as control:
                control.connect(str(runtime / 'control.sock'))
                control.sendall(b'bindings\n')
                response = control.makefile('rb').readline(1024)
                assert json.loads(response) == ['Ctrl+Shift+F10', 'Ctrl+Shift+F11']
                assert KEY.encode() not in response
            assert_private_files()
            # Forgetting while locked disables reuse and retains a cleanup reference.
            cli(['forget-key'], ok=False)
            assert 'credential_id' not in settings() and len(settings()['retired_credentials']) == 1
            assert count_credentials() == 1
            keyring.terminate()
            keyring.wait(timeout=5)
            keyring = subprocess.Popen([daemon, '--foreground', '--components=secrets', '--control-directory',
                                        str(root / 'control'), '--unlock'], env=env, stdin=subprocess.PIPE,
                                       stdout=log, stderr=log)
            owned.append(keyring)
            keyring.stdin.write(b'disposable-test-password\n')
            keyring.stdin.close()
            wait_for(keyring_ready, 15)
            cli(['forget-key'])
            assert count_credentials() == 0 and 'retired_credentials' not in settings()
            assert loaded.read_text() == str(base)
            assert not (runtime / 'restore-path.json').exists()
            assert API.count == 1, 'Only the one explicit synthetic trigger may call HTTP'
            print('PASS: isolated encrypted native keyring, hidden setup, prompt-free restart, saved timeout, scope binding, locked-store failure, provider restart, cancelled change, rollback cleanup, change rotation, foreground forget-key, locked session-only preserving saved setup, cleanup retry, no secret in files/argv/env, no paid API')
        finally:
            try:
                subprocess.run(args + ['stop'], env=env, stdin=subprocess.DEVNULL, capture_output=True, timeout=15)
            except (subprocess.TimeoutExpired, OSError):
                pass
            if server:
                server.shutdown()
                server.server_close()
            if watcher and watcher.stdin:
                watcher.stdin.close()
            for process in reversed(owned):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            log.close()


if __name__ == '__main__':
    main()
