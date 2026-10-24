"""Controlled JSON caller-turn evidence. Uses synthetic sessions, never phones.

Runs an isolated API child with an optional local OpenAI-compatible mock. Redis
DBs 10..14 are used exclusively for this run; do not use those DBs concurrently.
The API reads the repository .env for PostgreSQL. Credentials are never printed.
"""
import argparse
import base64
import concurrent.futures
import json
import hashlib
import hmac
import math
import os
import re
from pathlib import Path
import subprocess
import threading
import time
import urllib.request
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from uuid import uuid4


def request(base, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=15) as response:
        raw = response.read().decode()
        return json.loads(raw) if raw else None


def quantile(values, q):
    return sorted(values)[max(0, math.ceil(len(values) * q) - 1)] if values else None


def stage_means(text):
    stages = {}
    for line in text.splitlines():
        match = re.match(r'sayangcare_caller_turn_stage_duration_seconds_(sum|count)\{([^}]+)\} ([\d.eE+-]+)$', line)
        if not match:
            continue
        stage = re.search(r'stage="([^"]+)"', match[2])[1]
        values = stages.setdefault(stage, {'sum': 0.0, 'count': 0.0})
        values[match[1]] += float(match[3])
    return {stage: round(values['sum'] * 1000 / values['count'], 3)
            for stage, values in stages.items() if values['count']}


def webhook(base, path, parameters):
    signed = base + path + ''.join(key + parameters[key] for key in sorted(parameters))
    signature = base64.b64encode(hmac.new(b'test', signed.encode(), hashlib.sha1).digest()).decode()
    req = urllib.request.Request(base + path, data=urllib.parse.urlencode(parameters).encode(),
                                 headers={'Content-Type': 'application/x-www-form-urlencoded', 'X-Twilio-Signature': signature})
    with urllib.request.urlopen(req, timeout=15) as response:
        return response.read().decode()


class Mock(BaseHTTPRequestHandler):
    sentiment_ms = 20
    reply_ms = 50

    def log_message(self, *_):
        pass

    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        scoring = any('Return valid JSON only' in m.get('content', '') for m in payload['messages'])
        time.sleep((self.sentiment_ms if scoring else self.reply_ms) / 1000)
        content = json.dumps({'valence': 0.2, 'arousal': 0.3, 'distress_markers': []}) if scoring else 'I am listening. Tell me more.'
        body = json.dumps({'choices': [{'message': {'content': content}}]}).encode()
        try:
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass  # Deliberate client deadline cancellation.


def run(args, name, database, sentiment_ms, reply_ms, count, workers, use_ai, real_ai=False):
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    Mock.sentiment_ms, Mock.reply_ms = sentiment_ms, reply_ms
    server = ThreadingHTTPServer(('127.0.0.1', 0), Mock)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{args.port}'
    env = os.environ.copy()
    env.update({
        'SAYANGCARE__SERVER__HOST': '127.0.0.1',
        'SAYANGCARE__SERVER__PORT': str(args.port),
        'SAYANGCARE__REDIS__MASTER_URL': f'redis://127.0.0.1:6379/{database}',
        'SAYANGCARE__REDIS__QUEUE_URL': f'redis://127.0.0.1:6379/{database}',
        'SAYANGCARE__QUEUE__BACKEND': 'redis',
        'SAYANGCARE__CIRCUIT_BREAKER__KEY_PREFIX': 'sayangcare:perf:' + str(uuid4()),
        'SAYANGCARE__POSTGRES__MAX_CONNECTIONS': '4',
        'SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID': 'ACtest',
        'SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN': 'test',
        'SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL': base,
        'SAYANGCARE__AI__API_KEY': 'local-mock' if use_ai else '',
        'SAYANGCARE__AI__BASE_URL': f'http://127.0.0.1:{server.server_port}/chat/completions',
        'SAYANGCARE__PERFORMANCE__MAX_AI_TURNS': '2',
        'SAYANGCARE__PERFORMANCE__RESPONSE_BUDGET_MS': '5000',
        'SAYANGCARE__PERFORMANCE__PERSISTENCE_RESERVE_MS': '1000',
        'SAYANGCARE__PERFORMANCE__SENTIMENT_TIMEOUT_MS': '800',
        'SAYANGCARE__PERFORMANCE__REPLY_TIMEOUT_MS': '3000',
        'SAYANGCARE__PERFORMANCE__COORDINATION_TIMEOUT_MS': '200',
        'RUST_LOG': 'warn',
    })
    if real_ai:
        for key in ['SAYANGCARE__AI__API_KEY', 'SAYANGCARE__AI__BASE_URL']:
            if key in os.environ:
                env[key] = os.environ[key]
            else:
                env.pop(key, None)  # API loads real provider settings from .env.
    rows = []
    with (output / f'{name}.log').open('w', encoding='utf-8') as log:
        process = subprocess.Popen([str(Path(args.binary).resolve())], env=env, stdout=log, stderr=log)
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError(f'API exited; inspect {name}.log locally')
                try:
                    ready = request(base, '/api/v1/ready')
                    if ready.get('ready'):
                        break
                except Exception:
                    time.sleep(0.1)
            else:
                raise RuntimeError('API did not become ready')

            def lifecycle(_):
                sid = 'SIM-PERF-' + str(uuid4())
                request(base, '/api/v1/calls/incoming', {'call_sid': sid, 'from': '+6500000000', 'to': '+6500000001'})
                started = time.perf_counter()
                response = request(base, f'/api/v1/calls/{sid}/turn', {'transcript': 'Hello, I would like to chat.'})
                elapsed = (time.perf_counter() - started) * 1000
                session = request(base, f'/api/v1/calls/{sid}')
                assert len(session['transcript']['turns']) == 2, 'missing/duplicated transcript'
                assert session['transcript']['turns'][0]['text'] == 'Hello, I would like to chat.'
                request(base, f'/api/v1/calls/{sid}/hangup', {})
                # Tiered GET must find the archived record after hot deletion.
                archived = request(base, f'/api/v1/calls/{sid}')
                assert archived['state'] == 'completed'
                assert len(archived['transcript']['turns']) == 2
                return {'session_id': sid, 'turn_ms': elapsed, 'state': response['state'], 'audio_fallback': bool(response.get('fallback_audio_url'))}

            with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
                rows = list(pool.map(lifecycle, range(count)))
            with urllib.request.urlopen(base + '/api/v1/metrics', timeout=5) as response:
                metric_bytes = response.read()
                (output / f'{name}.prom').write_bytes(metric_bytes)
                if real_ai:
                    assert b'stage="sentiment"' in metric_bytes or b'stage="reply"' in metric_bytes, 'No model operation observed: check configured provider/key'
            if not real_ai:
                # Signed Twilio smoke checks run AFTER the measurement snapshot.
                sid = 'SIM-PERF-VOICE-' + str(uuid4())
                xml = webhook(base, '/twilio/voice', {'CallSid': sid, 'From': '+6500000000', 'To': '+6500000001'})
                if name == 'mock-slow-reply':
                    assert '<Play>' in xml and '<Gather ' in xml, 'incoming Open fallback missing'
                xml = webhook(base, '/twilio/gather', {'CallSid': sid, 'SpeechResult': 'I want to kill myself'})
                assert '<Play>' not in xml and '<Gather ' in xml, 'crisis triage must bypass audio/AI'
                escalated = request(base, f'/api/v1/calls/{sid}')
                assert escalated['state'] == 'escalated'
                alerts = request(base, '/api/v1/operator/alerts')
                assert any(alert['session_id'] == sid for alert in alerts['alerts']), 'shared operator alert missing'

                webhook(base, '/twilio/status', {'CallSid': sid, 'CallStatus': 'completed'})
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            server.shutdown()
            server.server_close()
    latencies = [row['turn_ms'] for row in rows]
    result = {'scenario': name, 'mode': 'real-ai' if real_ai else ('mock-ai' if use_ai else 'deterministic-no-ai'), 'calls': len(rows),
              'workers': workers, 'ai_limit_per_process': 2, 'response_budget_ms': 5000,
              'sentiment_mock_ms': sentiment_ms if use_ai and not real_ai else None, 'reply_mock_ms': reply_ms if use_ai and not real_ai else None,
              'p50_turn_ms': quantile(latencies, .5), 'p95_turn_ms': quantile(latencies, .95),
              'p99_turn_ms': quantile(latencies, .99), 'max_turn_ms': max(latencies),
              'audio_fallbacks': sum(row['audio_fallback'] for row in rows), 'rows': rows}
    result['stage_mean_ms'] = stage_means(metric_bytes.decode())
    (output / f'{name}.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps({k: v for k, v in result.items() if k != 'rows'}), flush=True)
    assert max(latencies) < 5500, 'turn exceeded 5s budget plus 500ms local scheduling allowance'
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/perf-validation/debug/sayangcare.exe' if os.name == 'nt' else 'target/perf-validation/debug/sayangcare')
    parser.add_argument('--port', type=int, default=18091)
    parser.add_argument('--output', default='artifacts/perf')
    parser.add_argument('--real-ai', action='store_true', help='Opt-in: three sequential synthetic turns against configured provider; uses API credits.')
    args = parser.parse_args()
    # Fail rather than accidentally benchmarking an already-running API.
    import socket
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', args.port))
    if args.real_ai:
        run(args, 'real-ai', 15, 0, 0, 3, 1, True, real_ai=True)
        return
    scenarios = [
        ('no-ai', 10, 0, 0, 20, 1, False),
        ('mock-fast', 11, 20, 50, 20, 1, True),
        ('mock-slow-sentiment', 12, 1200, 50, 5, 1, True),
        ('mock-slow-reply', 13, 20, 4000, 5, 1, True),
        ('mock-overload', 14, 20, 500, 16, 16, True),
    ]
    results = [run(args, *scenario) for scenario in scenarios]
    assert results[1]['audio_fallbacks'] == 0
    assert results[3]['audio_fallbacks'] > 0
    assert results[4]['audio_fallbacks'] > 0
    summary = [{k: v for k, v in r.items() if k != 'rows'} for r in results]
    Path(args.output, 'summary.json').write_text(json.dumps(summary, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
