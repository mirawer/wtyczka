#!/usr/bin/env python3
"""Integration test: fake grim+niri, local HTTP, real session D-Bus tray.
No screenshots, external API calls, or changes to compositor configuration.
All test files live in this project's .cache, not /tmp.
"""
import base64
import http.server
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import tempfile
import threading
import time
import termios

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get('SCREEN_ANSWER_BIN', ROOT / 'target/debug/screen-answer')).resolve()
# Only the JPEG signature is needed: the mock HTTP endpoint doesn't decode images.
FAKE_IMAGE = b'\xff\xd8\xffsynthetic-image-for-tests'


def wait_for(predicate, seconds=8):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.03)
    raise AssertionError('Timed out')


class API(http.server.BaseHTTPRequestHandler):
    count = 0
    models = []
    release = threading.Event()
    received = threading.Event()
    completed = threading.Event()
    output = '9'
    max_choices = 9
    multiple = False

    def do_POST(self):
        type(self).count += 1
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        type(self).models.append(body['model'])
        assert self.path == '/v1/messages'
        assert self.headers['x-api-key'] == 'test-secret-never-log'
        assert body['max_tokens'] == 4096
        assert 'thinking' not in body  # never disable mandatory Opus thinking
        enum = [str(n) for n in range(1, type(self).max_choices + 1)]
        if type(self).multiple:
            properties = {'answers': {'type': 'array', 'items': {'type': 'string', 'enum': enum}}}
            required = ['answers']
        else:
            properties = {'answer': {'type': 'string', 'enum': enum + ['?']}}
            required = ['answer']
        assert body['output_config']['format'] == {
            'type': 'json_schema',
            'schema': {'type': 'object', 'properties': properties, 'required': required, 'additionalProperties': False},
        }
        assert 'Return one JSON object' in body['system']
        assert base64.b64decode(body['messages'][0]['content'][0]['source']['data']) == FAKE_IMAGE
        output = type(self).output
        multiple = type(self).multiple
        type(self).received.set()
        assert type(self).release.wait(8)
        result = json.dumps({'stop_reason': 'end_turn', 'content': [
            {'type': 'thinking', 'thinking': '', 'signature': 'synthetic-signature'},
            {'type': 'text', 'text': json.dumps({('answers' if multiple else 'answer'): output})}
        ]}).encode()
        try:
            self.send_response(200)
            self.send_header('Content-Length', str(len(result)))
            self.end_headers()
            self.wfile.write(result)
        except (BrokenPipeError, ConnectionResetError):
            pass  # Expected when pause cancels a request already received by the mock.
        finally:
            type(self).completed.set()

    def log_message(self, *_):
        pass


def expected_cell_pixels(n, mode, prefixed=False):
    if mode == 'dots':
        centers = {
            1: [(16,16)], 2: [(10,16),(22,16)], 3: [(16,10),(10,22),(22,22)],
            4: [(10,10),(22,10),(10,22),(22,22)],
            5: [(16,6),(10,16),(22,16),(10,26),(22,26)],
            6: [(10,6),(22,6),(10,16),(22,16),(10,26),(22,26)],
        }[n]
        pixels = {(x,y) for cx,cy in centers for y in range(cy-3,cy+4)
                  for x in range(cx-3,cx+4) if (x-cx)**2 + (y-cy)**2 <= 9}
    else:
        glyphs = {
            1: [4,12,4,4,4,4,14], 2: [14,17,1,2,4,8,31], 3: [30,1,1,14,1,1,30],
            4: [2,6,10,18,31,2,2], 5: [31,16,16,30,1,1,30], 6: [14,16,16,30,17,17,14],
            7: [31,1,2,4,8,8,8], 8: [14,17,17,14,17,17,14], 9: [14,17,17,15,1,1,14],
        }
        pixels = {(9+col*3+dx, 5+row*3+dy) for row,bits in enumerate(glyphs[n])
                  for col in range(5) if bits & (1 << (4-col)) for dy in range(3) for dx in range(3)}
    if prefixed:
        pixels = {(x+4,y) for x,y in pixels}
        pixels |= {(x,y) for y in range(5,11) for x in range(3,6 if y < 9 else 5)}
    return pixels


def interactive_start(args, env, answers):
    """Drive the actual wizard in a terminal, matching each prompt in sequence."""
    pid, master = pty.fork()
    if pid == 0:
        os.execve(str(BINARY), args, env)
    transcript = b''
    pending = b''
    step = 0
    reaped = False
    deadline = time.monotonic() + 25
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
            pending += chunk
            if step < len(answers):
                marker, answer = answers[step]
                marker = marker.encode()
                if marker in pending:
                    pending = pending.split(marker, 1)[1]
                    if marker.startswith(b'Klucz API (ukryty):'):
                        assert termios.tcgetattr(master)[3] & termios.ECHO == 0, 'Key prompt printed before echo was disabled'
                    os.write(master, answer.encode() + b'\n')
                    step += 1
        else:
            raise AssertionError('Wizard timed out')
        _, status = os.waitpid(pid, 0)
        reaped = True
        assert os.waitstatus_to_exitcode(status) == 0, transcript
        assert step == len(answers), transcript
        assert b'test-secret-never-log' not in transcript, transcript
        assert b'test-invisible' not in transcript, transcript
        assert 'Działa w tle'.encode() in transcript, transcript
        return transcript.decode()
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
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), API)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix='smoke-', dir=ROOT / '.cache') as tmp:
            root = Path(tmp)
            fakebin = root / 'bin'
            fakebin.mkdir()
            runtime = root / 'state'
            base = root / 'base.kdl'
            base.write_text('// synthetic base config\n')
            loaded = root / 'loaded-path'
            (fakebin / 'niri').write_text('#!/usr/bin/env python3\nimport sys,os,json\nfrom pathlib import Path\nargs=sys.argv[1:]\nif args[:3]==["msg","-j","focused-output"]: print(json.dumps({"name":"FAKE-1"}))\nelif args[:3]==["msg","action","load-config-file"]: Path(os.environ["TEST_LOADED"]).write_text(args[-1])\n')
            captures = root / 'captures'
            (fakebin / 'grim').write_text('#!/usr/bin/env python3\nimport sys,os\nfrom pathlib import Path\nwith Path(os.environ["TEST_CAPTURES"]).open("a") as f: f.write("capture\\n")\nsys.stdout.buffer.write(' + repr(FAKE_IMAGE) + ')\n')
            for p in fakebin.iterdir():
                p.chmod(0o700)
            env = dict(os.environ, PATH=f'{fakebin}:{os.environ["PATH"]}', TEST_LOADED=str(loaded), TEST_CAPTURES=str(captures))
            args = [str(BINARY), '--runtime-dir', str(runtime)]
            startup = {'settings': {'provider': 'anthropic', 'model': 'test-model', 'base_url': f'http://127.0.0.1:{server.server_port}/v1', 'keybind': 'Print'}, 'timeout': 5, 'niri_config': str(base)}
            proc = subprocess.Popen(args + ['serve'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
            try:
                proc.stdin.write(b'test-secret-never-log\n' + json.dumps(startup).encode() + b'\n')
                proc.stdin.close()
                assert select.select([proc.stdout], [], [], 10)[0], 'No readiness response'
                line = proc.stdout.readline()
                if line != b'READY\n':
                    raise AssertionError(f'Start failed: {line!r}: {proc.stderr.read().decode()}')
                assert loaded.read_text().endswith('/niri-overlay.kdl')
                assert (runtime.stat().st_mode & 0o777) == 0o700
                assert (runtime / 'control.sock').stat().st_mode & 0o777 == 0o600
                assert 'Print repeat=false' in (runtime / 'niri-overlay.kdl').read_text()
                assert 'test-secret' not in (runtime / 'settings.json').read_text()

                def cli(command):
                    result = subprocess.run(args + [command], env=env, capture_output=True, timeout=12)
                    assert result.returncode == 0, result.stderr.decode()
                    return result.stdout

                # Read real D-Bus pixmaps to verify state updates, not just mocked functions.
                services = subprocess.check_output(['busctl', '--user', '--no-pager', '--no-legend', 'list'], text=True)
                service = next(line.split()[0] for line in services.splitlines() if line.startswith(f'org.kde.StatusNotifierItem-{proc.pid}-'))

                def pixmap():
                    return subprocess.check_output(['busctl', '--user', 'get-property', service, '/ScreenAnswer/00', 'org.kde.StatusNotifierItem', 'IconPixmap'])

                def tray_property(name, item_service=None):
                    target = item_service or service
                    bus, path = target if isinstance(target, tuple) else (target, '/ScreenAnswer/00')
                    return json.loads(subprocess.check_output(['busctl', '--user', '--json=short', 'get-property', bus, path, 'org.kde.StatusNotifierItem', name]))['data']

                def assert_watcher_has_cells(bus, count):
                    owner = json.loads(subprocess.check_output(['busctl', '--user', '--json=short', 'call',
                        'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus', 'GetNameOwner', 's', bus]))['data'][0]
                    expected = {owner + f'/ScreenAnswer/{slot:02}' for slot in range(count)}
                    def registered():
                        actual = json.loads(subprocess.check_output(['busctl', '--user', '--json=short', 'get-property',
                            'org.kde.StatusNotifierWatcher', '/StatusNotifierWatcher', 'org.kde.StatusNotifierWatcher', 'RegisteredStatusNotifierItems']))['data']
                        return expected <= set(actual)
                    wait_for(registered)
                    return [(bus, f'/ScreenAnswer/{slot:02}') for slot in range(count)]

                def assert_transparent_no_hover(item_service=None):
                    assert tray_property('Title', item_service) == ''
                    assert tray_property('ToolTip', item_service) == ['', [], '', '']
                    width, height, pixels = tray_property('IconPixmap', item_service)[0]
                    assert (width, height) == (32, 32)
                    pixels = [tuple(pixels[i:i+4]) for i in range(0, len(pixels), 4)]
                    assert set(pixels) == {(0, 0, 0, 0), (255, 255, 255, 255)}
                    return pixels

                idle = pixmap()
                assert_watcher_has_cells(service, 1)
                assert_transparent_no_hover()
                assert b'Program dzia' in cli('status')
                assert cli('trigger') == b''
                assert API.received.wait(5)
                busy = wait_for(lambda: (p if (p := pixmap()) != idle else None))
                # A second request while busy must not capture or send again.
                assert cli('trigger') == b''
                assert API.count == 1
                API.release.set()
                answer = wait_for(lambda: (p if (p := pixmap()) != busy else None))
                assert answer != idle
                assert API.count == 1

                # An invalid model response clears the old answer and produces a new error icon.
                API.output = '1 and 2'
                API.received.clear()
                time.sleep(0.8)  # deliberate debounce window, not subagent polling
                cli('trigger')
                assert API.received.wait(5)
                error = wait_for(lambda: (p if (p := pixmap()) not in (answer, busy) else None))
                assert error != idle
                assert API.count == 2

                # SIGTERM/stop restore the base and remove the socket.
                cli('stop')
                assert proc.wait(timeout=10) == 0
                assert loaded.read_text() == str(base)
                assert not (runtime / 'control.sock').exists()
                assert not (runtime / 'restore-path.json').exists()
                stdout, stderr = proc.stdout.read(), proc.stderr.read()
                assert stdout == b'', stdout
                assert b'test-secret' not in stderr
                assert b'1 and 2' not in stderr
                print('PASS: IPC, secret handling, capture adapter, mock HTTP, busy deduplication, real tray updates, error state, stop + restore')

                # Failure after UI/hotkey creation must still await native cleanup.
                failed_runtime = root / 'startup-failure'
                failed_runtime.mkdir(mode=0o700)
                (failed_runtime / 'settings.json').mkdir()
                failed = subprocess.run(
                    [str(BINARY), '--runtime-dir', str(failed_runtime), 'serve'],
                    input=b'test-secret-never-log\n' + json.dumps(startup).encode() + b'\n',
                    env=env, capture_output=True, timeout=12)
                assert failed.returncode != 0
                assert b'READY' not in failed.stdout
                assert b'test-secret' not in failed.stdout + failed.stderr
                assert loaded.read_text() == str(base)
                assert not (failed_runtime / 'restore-path.json').exists()
                assert not (failed_runtime / 'control.sock').exists()
                assert API.count == 2
                print('PASS: startup failure after tray creation restores binding, removes IPC, and never reports READY')

                # Exercise the public interactive launcher: hidden key entry, detach,
                # configurable binding, single-instance protection, crash recovery.
                pid, master = pty.fork()
                if pid == 0:
                    os.execve(str(BINARY), args + ['--provider', 'anthropic', '--model', 'test-model', '--base-url', startup['settings']['base_url'], '--niri-config', str(base), '--keybind', 'Ctrl+Shift+F8', '--pause-keybind', 'off', '--mode', 'digits', '--selection', 'single'], env)
                transcript = b''
                sent = False
                deadline = time.monotonic() + 25
                try:
                    while time.monotonic() < deadline:
                        if select.select([master], [], [], 0.2)[0]:
                            try:
                                chunk = os.read(master, 4096)
                            except OSError:
                                break
                            if not chunk:
                                break
                            transcript += chunk
                            if b'Klucz API (ukryty):' in transcript and not sent:
                                assert termios.tcgetattr(master)[3] & termios.ECHO == 0
                                os.write(master, b'test-secret-never-log\n')
                                sent = True
                    else:
                        raise AssertionError('Interactive launcher timed out')
                    _, status = os.waitpid(pid, 0)
                    assert os.waitstatus_to_exitcode(status) == 0, transcript
                    assert sent
                    assert b'test-secret-never-log' not in transcript, transcript
                    assert 'Działa w tle'.encode() in transcript, transcript
                    cli('status')
                    assert 'Ctrl+Shift+F8 repeat=false' in (runtime / 'niri-overlay.kdl').read_text()
                    duplicate = subprocess.run(args, env=env, capture_output=True, timeout=5)
                    assert duplicate.returncode != 0
                    assert b'Klucz API' not in duplicate.stdout
                    control = socket.socket(socket.AF_UNIX)
                    control.connect(str(runtime / 'control.sock'))
                    daemon_pid = struct.unpack('3i', control.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))[0]
                    control.close()
                    os.kill(daemon_pid, signal.SIGKILL)
                    # Wait for the process to release its file lock after SIGKILL.
                    import fcntl
                    def unlocked():
                        with (runtime / 'daemon.lock').open('r') as f:
                            try:
                                fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                                return True
                            except BlockingIOError:
                                return False
                    wait_for(unlocked)
                    cli('restore-keybind')
                    assert loaded.read_text() == str(base)
                    assert not (runtime / 'restore-path.json').exists()
                    assert b'test-secret' not in (runtime / 'daemon.log').read_bytes()
                    print('PASS: interactive key not echoed, background detach, --keybind, single instance, SIGKILL recovery')
                finally:
                    os.close(master)
                    subprocess.run(args + ['stop'], env=env, capture_output=True, timeout=15)

                # No provider/model/keybind flags: first run, remembered settings,
                # then a different provider and shortcut. No capture/API trigger.
                (runtime / 'settings.json').unlink()
                wizard_args = args + ['--niri-config', str(base)]
                cases = [
                    ([('Dostawca [', 'claude'), ('Model: ', 'wizard-model'), ('Skrót [', ''), ('Skrót pauzy [brak]: ', 'Ctrl+Shift+F9'), ('Odpowiedzi [', ''), ('Tryb [', ''),
                      ('Klucz API (ukryty): ', ''), ('Klucz API (ukryty): ', 'test-invisible\u200b-key'),
                      ('Klucz API (ukryty): ', 'test-secret-never-log')],
                     'anthropic', 'wizard-model', 'Print', 'https://api.anthropic.com/v1', 'Ctrl+Shift+F9'),
                    ([('Dostawca [', ''), ('Model [wizard-model]: ', ''), ('Skrót [Print]: ', ''), ('Skrót pauzy [Ctrl+Shift+F9]: ', ''), ('Odpowiedzi [', ''), ('Tryb [', ''),
                      ('Klucz API (ukryty): ', 'test-secret-never-log')],
                     'anthropic', 'wizard-model', 'Print', 'https://api.anthropic.com/v1', 'Ctrl+Shift+F9'),
                    ([('Dostawca [', '4'), ('Model: ', 'gemini-test'), ('Skrót [Print]: ', 'Ctrl+F8'), ('Skrót pauzy [Ctrl+Shift+F9]: ', 'off'), ('Odpowiedzi [', ''), ('Tryb [', ''),
                      ('Klucz API (ukryty): ', 'test-secret-never-log')],
                     'gemini', 'gemini-test', 'Ctrl+F8', 'https://generativelanguage.googleapis.com/v1beta', None),
                    ([('Dostawca [', '5'), ('Adres API: ', f'http://127.0.0.1:{server.server_port}/v1'),
                      ('Model: ', 'local-vision-model'), ('Skrót [Ctrl+F8]: ', 'PrintScreen'), ('Skrót pauzy [brak]: ', ''), ('Odpowiedzi [', ''), ('Tryb [', ''),
                      ('Klucz API (ukryty): ', 'test-secret-never-log')],
                     'openai', 'local-vision-model', 'Print', f'http://127.0.0.1:{server.server_port}/v1', None),
                ]
                try:
                    for answers, provider, model, keybind, base_url, pause in cases:
                        transcript = interactive_start(wizard_args, env, answers)
                        for step in ['1/7', '2/7', '3/7', '4/7', '5/7', '6/7', '7/7', '9 opcji (A–I)']:
                            assert step in transcript
                        actual = json.loads((runtime / 'settings.json').read_text())
                        expected = dict(provider=provider, model=model, keybind=keybind, base_url=base_url, mode='digits', selection='single')
                        if pause is not None:
                            expected['pause_keybind'] = pause
                        assert actual == expected
                        overlay = (runtime / 'niri-overlay.kdl').read_text()
                        assert ('"toggle-pause"' in overlay) == (pause is not None)
                        if pause is not None:
                            assert f'{pause} repeat=false' in overlay
                        cli('status')
                        cli('stop')
                        assert API.count == 2, 'Wizard must never capture or call an API'
                    print('PASS: wizard without setup flags, first-run defaults, saved choices, empty/invalid key retry without echo, provider/model/keybind changes, custom API URL')
                    # Reproduce the reported typo; choose Opus by number without a real API call.
                    (runtime / 'settings.json').write_text(json.dumps(dict(
                        provider='anthropic', model='claud-opus-5-5',
                        base_url='https://api.anthropic.com/v1', keybind='Print')))
                    transcript = interactive_start(wizard_args, env, [
                        ('Dostawca [', ''), ('Model: ', '3'), ('Skrót [Print]: ', ''), ('Skrót pauzy [brak]: ', ''), ('Odpowiedzi [', ''), ('Tryb [', ''),
                        ('Klucz API (ukryty): ', 'test-secret-never-log')])
                    assert '3. Claude Opus 5.5 — claude-opus-5-5' in transcript
                    assert 'Poprzednia nazwa modelu jest nieprawidłowa' in transcript
                    assert '4096' in transcript
                    assert json.loads((runtime / 'settings.json').read_text())['model'] == 'claude-opus-5-5'
                    cli('stop')
                    assert API.count == 2
                    print('PASS: typo regression, model examples, numbered Opus selection, token-budget notice; no paid requests')

                    # Exercise all three Claude IDs from launch through HTTP to the real tray.
                    # This validates the wire contract and parser, NOT the provider's generation.
                    for n, model in enumerate(['claude-haiku-5-5', 'claude-sonnet-5-5', 'claude-opus-5-5'], 1):
                        API.output = str(n)
                        API.received.clear()
                        count = API.count
                        interactive_start(wizard_args + [
                            '--provider', 'anthropic', '--model', model,
                            '--base-url', startup['settings']['base_url'], '--keybind', 'Print', '--pause-keybind', 'off', '--mode', 'digits', '--selection', 'single'], env,
                            [('Klucz API (ukryty): ', 'test-secret-never-log')])
                        control = socket.socket(socket.AF_UNIX)
                        control.connect(str(runtime / 'control.sock'))
                        daemon_pid = struct.unpack('3i', control.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))[0]
                        control.close()
                        services = subprocess.check_output(['busctl', '--user', '--no-pager', '--no-legend', 'list'], text=True)
                        service = next(line.split()[0] for line in services.splitlines() if line.startswith(f'org.kde.StatusNotifierItem-{daemon_pid}-'))
                        cli('trigger')
                        assert API.received.wait(5)
                        wait_for(lambda: (p if (p := pixmap()) not in (idle, busy, error) else None))
                        assert API.models[-1] == model
                        assert API.count == count + 1
                        assert_transparent_no_hover()
                        assert (runtime / 'daemon.log').read_bytes() == b''
                        cli('stop')
                    print('PASS: Haiku/Sonnet/Opus launch → schema-constrained mock HTTP → real tray digit, no error icon')

                    API.max_choices = 6
                    dot_args = wizard_args + ['--provider', 'anthropic', '--model', 'test-model',
                        '--base-url', startup['settings']['base_url'], '--keybind', 'Print', '--pause-keybind', 'off', '--selection', 'single']
                    interactive_start(dot_args, env, [('Tryb [', '2'), ('Klucz API (ukryty): ', 'test-secret-never-log')])
                    assert json.loads((runtime / 'settings.json').read_text())['mode'] == 'dots'
                    control = socket.socket(socket.AF_UNIX)
                    control.connect(str(runtime / 'control.sock'))
                    daemon_pid = struct.unpack('3i', control.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))[0]
                    control.close()
                    services = subprocess.check_output(['busctl', '--user', '--no-pager', '--no-legend', 'list'], text=True)
                    service = next(line.split()[0] for line in services.splitlines() if line.startswith(f'org.kde.StatusNotifierItem-{daemon_pid}-'))
                    for value, centers in [('5', [(16,6), (10,16), (22,16), (10,26), (22,26)]),
                                           ('6', [(10,6), (22,6), (10,16), (22,16), (10,26), (22,26)])]:
                        API.output = value
                        API.received.clear()
                        cli('trigger')
                        assert API.received.wait(5)
                        wait_for(lambda: (p if (p := pixmap()) not in (idle, busy, error) else None))
                        pixels = assert_transparent_no_hover()
                        expected = {(x,y) for cx,cy in centers for y in range(cy-3,cy+4)
                                    for x in range(cx-3,cx+4) if (x-cx)**2 + (y-cy)**2 <= 9}
                        assert {(i%32,i//32) for i,p in enumerate(pixels) if p[0]} == expected
                        time.sleep(0.8)  # wait for the deliberate hotkey debounce, not an agent
                    API.output = '7'  # server violates its schema: never show a 7 in six-choice mode
                    API.received.clear()
                    cli('trigger')
                    assert API.received.wait(5)
                    wait_for(lambda: pixmap() == error)
                    cli('stop')
                    assert 'limitem 6 opcji' in (runtime / 'daemon.log').read_text()
                    print('PASS: dots mode 1–6, exact five/six dot layouts, transparent pixels, no hover metadata, rejects 7')

                    API.multiple = True
                    for display, max_choices in [('digits', 9), ('dots', 6)]:
                        API.max_choices = max_choices
                        multi_args = wizard_args + ['--provider', 'anthropic', '--model', 'test-model',
                            '--base-url', startup['settings']['base_url'], '--keybind', 'Print', '--pause-keybind', 'Ctrl+Shift+F9']
                        transcript = interactive_start(multi_args, env, [
                            ('Odpowiedzi [', '2'), ('Tryb [', '1' if display == 'digits' else '2'),
                            ('Klucz API (ukryty): ', 'test-secret-never-log')])
                        assert transcript.index('Odpowiedzi [') < transcript.index('Tryb [')
                        settings = json.loads((runtime / 'settings.json').read_text())
                        assert settings['selection'] == 'multiple' and settings['mode'] == display
                        assert settings['pause_keybind'] == 'Ctrl+Shift+F9'
                        assert 'Ctrl+Shift+F9 repeat=false' in (runtime / 'niri-overlay.kdl').read_text()
                        control = socket.socket(socket.AF_UNIX)
                        control.connect(str(runtime / 'control.sock'))
                        daemon_pid = struct.unpack('3i', control.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))[0]
                        control.close()
                        services = subprocess.check_output(['busctl', '--user', '--no-pager', '--no-legend', 'list'], text=True)
                        service = next(line.split()[0] for line in services.splitlines() if line.startswith(f'org.kde.StatusNotifierItem-{daemon_pid}-'))
                        cell_services = assert_watcher_has_cells(service, max_choices)
                        cells = {}
                        for cell in cell_services:
                            identity = tray_property('Id', cell)
                            slot = 0 if identity == 'screen-answer' else int(identity.rsplit('-', 1)[1]) - 1
                            cells[slot] = cell
                        service = cells[0][0]

                        def active_slots():
                            return sorted(slot for slot, cell in cells.items() if tray_property('Status', cell) == 'Active')

                        assert active_slots() == [0]
                        API.output = ['2', '1']  # normalize to 1'2, not response order
                        API.received.clear()
                        API.release.clear()
                        cli('trigger')
                        assert API.received.wait(5)
                        assert active_slots() == [0]
                        assert pixmap() == busy
                        API.release.set()
                        wait_for(lambda: active_slots() == [0, 1])
                        for slot, value in enumerate([1,2]):
                            pixels = assert_transparent_no_hover(cells[slot])
                            assert {(i%32,i//32) for i,p in enumerate(pixels) if p[0]} == expected_cell_pixels(value, display, slot > 0)
                        time.sleep(0.8)  # hotkey debounce

                        # Longest possible answer must not be squeezed, truncated or duplicated.
                        API.output = [str(n) for n in range(1, max_choices+1)]
                        API.received.clear()
                        cli('trigger')
                        assert API.received.wait(5)
                        wait_for(lambda: active_slots() == list(range(max_choices)))
                        for slot in range(max_choices):
                            pixels = assert_transparent_no_hover(cells[slot])
                            assert {(i%32,i//32) for i,p in enumerate(pixels) if p[0]} == expected_cell_pixels(slot+1, display, slot > 0)
                        time.sleep(0.8)

                        # Pausing a full answer clears every trailing cell and shows the exact idle dash.
                        cli('toggle-pause')
                        wait_for(lambda: active_slots() == [0] and pixmap() == idle)
                        assert_transparent_no_hover(cells[0])
                        count = API.count
                        captured = captures.read_bytes()
                        API.received.clear()
                        cli('trigger')
                        cli('trigger')
                        assert not API.received.wait(0.2), 'Paused trigger must not send an API request'
                        assert API.count == count and captures.read_bytes() == captured
                        assert pixmap() == idle

                        # Resume is idle, not an automatic capture. Then cancel an in-flight request.
                        cli('toggle-pause')
                        assert API.count == count and captures.read_bytes() == captured
                        API.output = ['4']
                        API.received.clear()
                        API.completed.clear()
                        API.release.clear()
                        cli('trigger')
                        assert API.received.wait(5)
                        assert pixmap() == busy
                        cli('toggle-pause')
                        wait_for(lambda: active_slots() == [0] and pixmap() == idle)
                        count = API.count
                        captured = captures.read_bytes()
                        cli('toggle-pause')
                        API.release.set()  # The old server answer arrives AFTER cancellation and resume.
                        assert API.completed.wait(5)
                        assert active_slots() == [0] and pixmap() == idle
                        assert API.count == count and captures.read_bytes() == captured

                        # A fresh trigger after resume works without replaying old work.
                        API.output = ['3']
                        API.received.clear()
                        cli('trigger')
                        assert API.received.wait(5)
                        wait_for(lambda: active_slots() == [0] and pixmap() not in (busy, error, idle))
                        pixels = assert_transparent_no_hover(cells[0])
                        assert {(i%32,i//32) for i,p in enumerate(pixels) if p[0]} == expected_cell_pixels(3, display)
                        time.sleep(0.8)

                        API.output = ['1', '1']  # malformed response: no partial or guessed display
                        API.received.clear()
                        cli('trigger')
                        assert API.received.wait(5)
                        wait_for(lambda: active_slots() == [0] and pixmap() == error)
                        cli('stop')
                        assert loaded.read_text() == str(base)
                    print('PASS: multiple-selection wizard, 1\'2 digits/dots, full-size cells up to 9/6 answers, pause/resume to identical idle dash, no paused capture/API, cancel late result, duplicate rejection, stop')
                finally:
                    subprocess.run(args + ['stop'], env=env, capture_output=True, timeout=15)
            finally:
                API.release.set()
                if proc.poll() is None:
                    proc.terminate()
                    try:
                        proc.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        proc.kill()
                        proc.wait()
    finally:
        server.shutdown()
        server.server_close()


if __name__ == '__main__':
    main()
