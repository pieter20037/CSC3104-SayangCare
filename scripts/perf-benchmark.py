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

LOCAL_HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def real_ai_settings(environ, env_file):
    settings = {}
    if env_file.exists():
        for line in env_file.read_text(encoding='utf-8-sig').splitlines():
            match = re.match(r'^\s*(SAYANGCARE__AI__(?:API_KEY|BASE_URL|MODEL))\s*=\s*(.*?)\s*$', line)
            if match:
                settings[match[1]] = match[2].strip('"\'')
    settings.update({key: value for key, value in environ.items() if key.startswith('SAYANGCARE__AI__')})
    if not settings.get('SAYANGCARE__AI__API_KEY', '').strip():
        raise ValueError('Real AI test blocked: configure SAYANGCARE__AI__API_KEY locally; do not paste it into chat.')
    endpoint = settings.get('SAYANGCARE__AI__BASE_URL', 'https://api.groq.com/openai/v1/chat/completions')
    parsed = urllib.parse.urlparse(endpoint)
    if parsed.scheme != 'https' or parsed.hostname != 'api.groq.com' or parsed.username or parsed.password:
        raise ValueError('Live test only sends credentials to the configured HTTPS api.groq.com endpoint.')
    settings['SAYANGCARE__AI__BASE_URL'] = endpoint
    return settings


def request(base, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, headers={'Content-Type': 'application/json'})
    with LOCAL_HTTP.open(req, timeout=15) as response:
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


def stage_outcomes(text):
    outcomes = {}
    for line in text.splitlines():
        match = re.match(r'sayangcare_caller_turn_stage_duration_seconds_count\{([^}]+)\} ([\d.eE+-]+)$', line)
        if not match:
            continue
        labels = dict(re.findall(r'(\w+)="([^"]*)"', match[1]))
        if labels.get('path') != 'json' or labels.get('stage') not in ('sentiment', 'reply'):
            continue
        stage = outcomes.setdefault(labels['stage'], {})
        outcome = labels['outcome']
        stage[outcome] = stage.get(outcome, 0) + int(float(match[2]))
    return outcomes


def validate_real_ai(text, rows):
    outcomes = stage_outcomes(text)
    assert outcomes.get('sentiment', {}).get('ok', 0) > 0, 'No successful sentiment operation; real AI validation failed'
    assert outcomes.get('reply', {}).get('ok', 0) > 0, 'No successful reply operation; real AI validation failed'
    assert any(not row['audio_fallback'] for row in rows), 'All real AI turns used audio fallback'
    for stage in ('sentiment', 'reply'):
        assert outcomes[stage].get('ok', 0) == len(rows), f'Not every live turn had successful {stage}'
        assert all(count == 0 for outcome, count in outcomes[stage].items() if outcome != 'ok'), f'Live {stage} failures remain'
    assert all(not row['audio_fallback'] for row in rows), 'Live turns used audio fallback'
    return outcomes


def webhook(base, path, parameters):
    signed = base + path + ''.join(key + parameters[key] for key in sorted(parameters))
    signature = base64.b64encode(hmac.new(b'test', signed.encode(), hashlib.sha1).digest()).decode()
    req = urllib.request.Request(base + path, data=urllib.parse.urlencode(parameters).encode(),
                                 headers={'Content-Type': 'application/x-www-form-urlencoded', 'X-Twilio-Signature': signature})
    with LOCAL_HTTP.open(req, timeout=15) as response:
        return response.read().decode()


class Mock(BaseHTTPRequestHandler):
    sentiment_ms = 20
    reply_ms = 50
    fail_reply = False

    def log_message(self, *_):
        pass

    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        scoring = any('Return valid JSON only' in m.get('content', '') for m in payload['messages'])
        time.sleep((self.sentiment_ms if scoring else self.reply_ms) / 1000)
        if self.fail_reply and not scoring:
            self.send_response(503)
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
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


def run(args, name, database, sentiment_ms, reply_ms, count, workers, use_ai, real_ai=False, recovery=False):
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    Mock.sentiment_ms, Mock.reply_ms = sentiment_ms, reply_ms
    Mock.fail_reply = False
    live_settings = real_ai_settings(os.environ, Path('.env')) if real_ai else {}
    server = ThreadingHTTPServer(('127.0.0.1', 0), Mock)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f'http://127.0.0.1:{args.port}'
    env = os.environ.copy()
    env.update({
        'SAYANGCARE__SERVER__HOST': '127.0.0.1',
        'SAYANGCARE__SERVER__PORT': str(args.port),
        'SAYANGCARE__REDIS__MASTER_URL': f'{args.redis_url.rstrip("/")}/{database}',
        'SAYANGCARE__REDIS__QUEUE_URL': f'{args.redis_url.rstrip("/")}/{database}',
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
        env.update(live_settings)
    rows = []
    recovery_evidence = []
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
                if real_ai and _ > 0:
                    time.sleep(args.real_ai_interval)
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

            def snapshot(phase):
                with LOCAL_HTTP.open(base + '/api/v1/metrics', timeout=5) as response:
                    text = response.read().decode()
                states = {breaker: int(value) for breaker, value in re.findall(
                    r'sayangcare_circuit_breaker_state\{breaker="([^"]+)"\} ([012])', text)}
                recovery_evidence.append({'phase': phase, 'elapsed_seconds': time.perf_counter() - recovery_started,
                                          'states': states})
                (output / f'{name}-transitions.json').write_text(json.dumps(recovery_evidence, indent=2), encoding='utf-8')
                return states

            if recovery:
                recovery_started = time.perf_counter()
                with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
                    baseline = list(pool.map(lifecycle, range(3)))
                    assert all(not row['audio_fallback'] for row in baseline), 'baseline inference unavailable'
                    assert snapshot('baseline')['llm-reply'] == 0
                    Mock.fail_reply = True
                    outage = list(pool.map(lifecycle, range(30)))
                    assert any(row['audio_fallback'] for row in outage), 'outage did not use audio fallback'
                    assert snapshot('outage')['llm-reply'] == 1, 'reply breaker did not open'
                    Mock.fail_reply = False
                    Mock.reply_ms = 300
                    time.sleep(11)  # Default half-open interval is ten seconds.
                    probe = pool.submit(lifecycle, 0)
                    while not probe.done():
                        snapshot('recovery-probe')
                        time.sleep(0.05)
                    recovered = probe.result()
                    assert not recovered['audio_fallback'], 'restored dependency still used fallback'
                    assert snapshot('restored')['llm-reply'] == 0
                    assert any(item['states'].get('llm-reply') == 2 for item in recovery_evidence), 'half-open probe not observed'
                    rows = baseline + outage + [recovered]
                (output / f'{name}-transitions.json').write_text(json.dumps(recovery_evidence, indent=2), encoding='utf-8')
            else:
                with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
                    rows = list(pool.map(lifecycle, range(count)))
            with LOCAL_HTTP.open(base + '/api/v1/metrics', timeout=5) as response:
                metric_bytes = response.read()
                (output / f'{name}.prom').write_bytes(metric_bytes)
            if not real_ai and not recovery:
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
    result['ai_stage_outcomes'] = stage_outcomes(metric_bytes.decode())
    result['mode'] = 'mock-dependency-recovery' if recovery else result['mode']
    result['validation_passed'] = False
    if real_ai:
        result['provider_host'] = urllib.parse.urlparse(live_settings['SAYANGCARE__AI__BASE_URL']).hostname
        result['model'] = live_settings.get('SAYANGCARE__AI__MODEL', 'allam-2-7b')
    (output / f'{name}.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    if real_ai:
        validate_real_ai(metric_bytes.decode(), rows)
    assert max(latencies) < 5500, 'turn exceeded 5s budget plus 500ms local scheduling allowance'
    result['validation_passed'] = True
    (output / f'{name}.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps({k: v for k, v in result.items() if k != 'rows'}), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/perf-validation/debug/sayangcare.exe' if os.name == 'nt' else 'target/perf-validation/debug/sayangcare')
    parser.add_argument('--port', type=int, default=18091)
    parser.add_argument('--output', default='artifacts/perf')
    parser.add_argument('--redis-url', default='redis://127.0.0.1:6379', help='Redis origin without DB suffix; reserved test DBs 10..15.')
    parser.add_argument('--real-ai', action='store_true', help='Opt-in: three sequential synthetic turns against configured provider; uses API credits.')
    parser.add_argument('--real-ai-calls', type=int, default=3, help='Sequential live sample count, 1..20; not a load benchmark.')
    parser.add_argument('--real-ai-interval', type=float, default=5, help='Seconds between live calls; minimum 5 to respect free-plan rate limits.')
    parser.add_argument('--check-real-ai', action='store_true', help='Check local credential/endpoint configuration without network requests.')
    parser.add_argument('--breaker-recovery', action='store_true', help='Isolated mock inference failure/restoration with measured breaker transitions.')
    args = parser.parse_args()
    if not 1 <= args.real_ai_calls <= 20 or args.real_ai_interval < 5:
        parser.error('Live tests require 1..20 calls and an interval of at least five seconds.')
    if args.real_ai and args.breaker_recovery:
        parser.error('--real-ai and --breaker-recovery are separate experiments')
    if args.real_ai or args.check_real_ai:
        try:
            real_ai_settings(os.environ, Path('.env'))
        except ValueError as error:
            parser.error(str(error))
    if args.check_real_ai:
        print('Real AI configuration present; no provider request was made.')
        return
    # Fail rather than accidentally benchmarking an already-running API.
    import socket
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', args.port))
    if args.real_ai:
        run(args, 'real-ai', 15, 0, 0, args.real_ai_calls, 1, True, real_ai=True)
        return
    if args.breaker_recovery:
        run(args, 'breaker-recovery', 14, 20, 50, 34, 2, True, recovery=True)
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
