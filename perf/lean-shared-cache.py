#!/usr/bin/env python3
"""Exercise native Lake sharing on a disposable Linux/ext4 directory.

PATH=/opt/lean/bin:$PATH python3 lean-shared-cache.py /experiment
No database, network cache, or MathDoc service is used. Results stay in a new
subdirectory; the caller owns cleanup. Requires Python 3, Lean/Lake, util-linux.
"""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time


def run(args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, check=True, timeout=120, **kwargs)


def files(root):
    return [p for p in root.rglob('*') if p.is_file()]


def hashes(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in files(root)}


def allocated(roots):
    # Count actual allocated blocks once per inode, including hard-linked ILeans.
    seen = {}
    for root in roots:
        for p in files(root):
            s = p.stat()
            seen[(s.st_dev, s.st_ino)] = s.st_blocks * 512
    return sum(seen.values())


class Lsp:
    """Small stdlib client for the same native requests MathDoc uses."""
    def __init__(self, workspace, env):
        self.workspace = workspace
        self.sequence = 0
        self.messages = queue.Queue()
        self.diagnostics = {}
        self.log = (workspace / 'lsp.log').open('w')
        self.process = subprocess.Popen(['lake', 'serve'], cwd=workspace, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log, start_new_session=True)
        threading.Thread(target=self.read, daemon=True).start()
        try:
            self.request('initialize', {'processId': None, 'rootUri': workspace.as_uri(),
                'capabilities': {'textDocument': {'publishDiagnostics': {'versionSupport': True}},
                                 'experimental': {'silentDiagnosticSupport': True}}})
            self.send({'method': 'initialized', 'params': {}})
        except BaseException:
            self.close()
            raise

    def read(self):
        try:
            while True:
                headers = {}
                while True:
                    line = self.process.stdout.readline()
                    if not line:
                        raise EOFError('Lean server closed stdout')
                    if line == b'\r\n':
                        break
                    key, value = line.decode().split(':', 1)
                    headers[key.lower()] = value.strip()
                size = int(headers['content-length'])
                data = self.process.stdout.read(size)
                self.messages.put(json.loads(data))
        except BaseException as error:
            self.messages.put(error)

    def send(self, message):
        payload = json.dumps({'jsonrpc': '2.0', **message}).encode()
        self.process.stdin.write(f'Content-Length: {len(payload)}\r\n\r\n'.encode() + payload)
        self.process.stdin.flush()

    def request(self, method, params):
        self.sequence += 1
        request_id = self.sequence
        self.send({'id': request_id, 'method': method, 'params': params})
        deadline = time.monotonic() + 90
        while True:
            message = self.messages.get(timeout=max(.01, deadline - time.monotonic()))
            if isinstance(message, BaseException):
                raise message
            if message.get('id') == request_id and 'method' not in message:
                assert 'error' not in message, message
                return message.get('result')
            if message.get('method') == 'textDocument/publishDiagnostics':
                p = message['params']
                old_version, previous = self.diagnostics.get(p['uri'], (None, []))
                if old_version != p.get('version') or not p.get('isIncremental'):
                    previous = []
                self.diagnostics[p['uri']] = (p.get('version'), previous + p['diagnostics'])
            elif 'id' in message and 'method' in message:
                result = [None] * len(message['params']['items']) if message['method'] == 'workspace/configuration' else None
                self.send({'id': message['id'], 'result': result})

    def check(self, value, version=1, expected_error=False):
        uri = (self.workspace / 'Lib/Editor.lean').as_uri()
        source = f'import Lib.A\ntheorem editorProof : value = {value} := by\n  rfl\n'
        if version == 1:
            (self.workspace / 'Lib/Editor.lean').write_text(source)
            self.send({'method': 'textDocument/didOpen', 'params': {'textDocument': {
                'uri': uri, 'languageId': 'lean4', 'version': version, 'text': source}}})
        else:
            self.send({'method': 'textDocument/didChange', 'params': {
                'textDocument': {'uri': uri, 'version': version}, 'contentChanges': [{'text': source}]}})
        self.request('textDocument/waitForDiagnostics', {'uri': uri, 'version': version})
        actual_version, diagnostics = self.diagnostics.get(uri, (None, []))
        assert actual_version == version, (actual_version, version, diagnostics)
        errors = [d for d in diagnostics if d.get('severity') == 1]
        assert bool(errors) == expected_error, diagnostics
        module = self.request('$/lean/prepareModuleHierarchy', {'textDocument': {'uri': uri}})
        imports = self.request('$/lean/moduleHierarchy/imports', {'module': module})
        assert any(i['module']['name'] == 'Lib.A' for i in imports), imports
        goals = self.request('$/lean/plainGoal', {'textDocument': {'uri': uri},
                            'position': {'line': 2, 'character': 2}})
        assert goals and goals.get('goals'), goals
        return {'errors': len(errors), 'goals': goals['goals']}

    def close(self):
        if self.log.closed:
            return
        try:
            if self.process.poll() is None:
                try:
                    self.request('shutdown', None)
                    self.send({'method': 'exit', 'params': None})
                    self.process.wait(timeout=5)
                except (Exception, KeyboardInterrupt):
                    pass
        finally:
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.process.wait(timeout=5)
            self.log.close()


def main():
    root = Path(tempfile.mkdtemp(prefix='lake-sharing-', dir=sys.argv[1])).resolve()
    cache = root / 'pool'
    events = root / 'compilations'
    events.write_text('')
    env = os.environ.copy()
    env.update(LAKE_ARTIFACT_CACHE='true', LAKE_RESTORE_ARTIFACTS='false',
               LAKE_CACHE_DIR=str(cache), LAKE_NO_CACHE='true', MATHLIB_NO_CACHE_ON_UPDATE='1')
    report = {'root': str(root), 'lean': run(['lean', '--version']).stdout.strip(),
              'lake': run(['lake', '--version']).stdout.strip(), 'platform': run(['uname', '-sm']).stdout.strip(),
              'filesystem': run(['findmnt', '-T', str(root), '-no', 'FSTYPE']).stdout.strip()}
    print(json.dumps(report), flush=True)
    assert report['filesystem'] == 'ext4', report
    testfile = root / 'reflink-source'
    testfile.write_bytes(b'x' * 4096)
    clone = subprocess.run(['cp', '--reflink=always', str(testfile), str(root/'reflink-copy')], capture_output=True, text=True)
    report['reflink'] = {'supported': clone.returncode == 0, 'message': clone.stderr.strip()}
    assert clone.returncode != 0, 'Test must not rely on reflink'

    def record(name, value):
        report[name] = value
        (root/'results.json').write_text(json.dumps(report, indent=2))
        print(json.dumps({name: value}), flush=True)

    def workspace(name, value=1, delay=0, modular=False):
        w = root/name
        (w/'Lib').mkdir(parents=True)
        (w/'lakefile.toml').write_text('name = "SharingProbe"\n[[lean_lib]]\nname = "Lib"\n')
        def marker(module):
            return ('run_cmd Lean.Elab.Command.liftIO do\n'
                    f'  let h ← IO.FS.Handle.mk {json.dumps(str(events))} IO.FS.Mode.append\n'
                    f'  h.putStrLn "{module}:{value}"\n  h.flush\n' +
                    (f'  IO.sleep {delay}\n' if delay and module == 'A' else ''))
        header = 'module\npublic ' if modular else ''
        public = 'public ' if modular else ''
        expose = '@[expose] ' if modular else ''
        (w/'Lib/A.lean').write_text(header+'import Lean\n'+marker('A')+expose+public+f'def value : Nat := {value}\n'+
                                   ''.join(public+f'def datum{i} : Nat := {i}\n' for i in range(1500)))
        (w/'Lib/B.lean').write_text(header+'import Lib.A\n'+marker('B')+public+f'theorem checked : value = {value} := rfl\n')
        return w

    def build(w, local_env=None):
        start = time.monotonic()
        p = subprocess.run(['lake', 'build', '+Lib.B'], cwd=w, env=local_env or env,
                           capture_output=True, text=True, timeout=120)
        (w/'build.log').write_text(p.stdout+p.stderr)
        assert p.returncode == 0, p.stdout+p.stderr
        return round(1000*(time.monotonic()-start), 1)

    base = workspace('baseline')
    record('cold_ms', build(base))
    assert events.read_text().splitlines() == ['A:1', 'B:1']
    baseline_hashes = hashes(cache/'artifacts')
    record('pool_baseline', {'files':len(baseline_hashes), 'allocated_bytes':allocated([cache])})
    events.write_text('')
    branches = [workspace(f'agent-{i}') for i in range(4)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        warm = list(executor.map(build, branches))
    assert events.read_text() == '', 'Warm branches compiled instead of reusing'
    cache_inodes = {(p.stat().st_dev, p.stat().st_ino) for p in files(cache/'artifacts')}
    for w in branches:
        assert not list((w/'.lake/build').rglob('*.olean')), 'Dependencies were restored unnecessarily'
        for p in (w/'.lake/build').rglob('*.ilean'):
            assert (p.stat().st_dev, p.stat().st_ino) in cache_inodes, p
    record('warm_four_agents', {'ms':warm, 'compilations':0,
        'private_olean_files':0, 'ilean_hardlinks':True,
        'additional_allocated_bytes':allocated([cache, *[w/'.lake' for w in branches]])-allocated([cache])})

    changed = workspace('changed', 2)
    record('changed_ms', build(changed))
    assert events.read_text().splitlines() == ['A:2', 'B:2']
    current = hashes(cache/'artifacts')
    assert all(current.get(k) == v for k,v in baseline_hashes.items()), 'Old objects were overwritten'
    record('changed_branch_isolated', True)
    events.write_text('')

    # Two independent LSP sessions import different versions from the same pool.
    reader = workspace('reader-original')
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
        servers = list(executor.map(lambda w: Lsp(w, env), [reader, changed]))
    try:
        record('lsp_original', servers[0].check(1))
        assert not list((reader/'.lake/build').rglob('*.olean'))
        record('lsp_reads_pool_without_local_olean', True)
        record('lsp_changed', servers[1].check(2))
        record('lsp_rejects_other_branch_value', servers[0].check(2, 2, expected_error=True))
        record('lsp_edit_recovers', servers[0].check(1, 3))
        servers[1].close()
        shutil.rmtree(changed)
        shutil.rmtree(base)
        record('lsp_survives_other_session_and_branch_removal', servers[0].check(1, 4))
        fresh = workspace('after-deletion')
        record('after_deletion_ms', build(fresh))
        assert events.read_text() == '', 'Deleting a producer lost shared artifacts'
    finally:
        for server in servers:
            server.close()

    # Native Lake has no cross-process single-flight for an initially missing key.
    events.write_text('')
    cold_env = {**env, 'LAKE_CACHE_DIR':str(root/'cold-race-pool')}
    racing = [workspace(f'race-{i}', 7, delay=1200) for i in range(4)]
    barrier = threading.Barrier(4)
    def race(w):
        barrier.wait()
        return build(w, cold_env)
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        durations = list(executor.map(race, racing))
    counts = events.read_text().splitlines()
    for p in (root/'cold-race-pool/outputs').rglob('*.json'):
        json.loads(p.read_text())
    record('cold_same_input_race', {'ms':durations, 'compiler_events':counts,
                                  'final_output_json_valid':True})

    # Reclaim only objects exclusive to value=2, after stopping its reader.
    current = hashes(cache/'artifacts')
    exclusive = set(current)-set(baseline_hashes)
    for key in exclusive:
        (cache/'artifacts'/key).unlink()
    events.write_text('')
    record('original_after_exclusive_objects_removed_ms', build(workspace('after-prune')))
    assert events.read_text() == ''
    record('missing_objects_rebuild_ms', build(workspace('recover-pruned', 2)))
    assert 'A:2' in events.read_text().splitlines()
    record('reclaimed_exclusive_objects', len(exclusive))

    # New module mode has split olean and IR outputs; let Lake enumerate them.
    modern = workspace('module-producer', 3, modular=True)
    build(modern)
    events.write_text('')
    modern_reader = workspace('module-reader', 3, modular=True)
    record('module_mode_reuse_ms', build(modern_reader))
    assert events.read_text() == ''
    assert not list((modern_reader/'.lake/build').rglob('*.olean*'))
    record('module_mode_shared_extensions', sorted({''.join(p.suffixes) for p in files(cache/'artifacts')}))

    # A cache hit can read a read-only pool; a miss is NOT a private write overlay.
    pool_files = files(cache)
    pool_dirs = [cache, *[p for p in cache.rglob('*') if p.is_dir()]]
    modes = {p:p.stat().st_mode & 0o777 for p in [*pool_files, *pool_dirs]}
    try:
        for p in pool_files:
            p.chmod(0o444)
        for p in pool_dirs:
            p.chmod(0o555)
        events.write_text('')
        record('readonly_pool_hit_ms', build(workspace('readonly-hit')))
        assert events.read_text() == ''
        missing = workspace('readonly-miss', 9)
        result = subprocess.run(['lake','build','+Lib.B'], cwd=missing, env=env,
                                capture_output=True, text=True, timeout=120)
        (missing/'build.log').write_text(result.stdout+result.stderr)
        assert result.returncode != 0, 'Unexpected automatic write overlay'
        assert 'permission denied' in (result.stdout+result.stderr).lower(), result
        record('readonly_pool_miss', {'exit_code':result.returncode, 'permission_denied':True})
    finally:
        for p, mode in modes.items():
            p.chmod(mode)
    record('complete', True)


if __name__ == '__main__':
    main()
