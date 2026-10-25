import http from 'k6/http';
import { check, sleep } from 'k6';
import { Counter, Rate } from 'k6/metrics';

const baseUrl = (__ENV.BASE_URL || 'http://127.0.0.1:18080').replace(/\/$/, '');
const api = `${baseUrl}/api/v1/calls`;
const vuCount = Number(__ENV.VUS || 5);
const duration = __ENV.DURATION || '30s';
const runId = __ENV.RUN_ID || `${Date.now()}`;
const archiveReadMaxAttempts = Number(__ENV.ARCHIVE_READ_MAX_ATTEMPTS || 3);
const callerTranscript = 'I have been feeling overwhelmed by a busy day.';

if (!Number.isInteger(vuCount) || vuCount < 1) {
  throw new Error('VUS must be a positive integer.');
}
if (!Number.isInteger(archiveReadMaxAttempts) || archiveReadMaxAttempts < 1) {
  throw new Error('ARCHIVE_READ_MAX_ATTEMPTS must be a positive integer.');
}

const completedLifecycles = new Rate('call_lifecycle_success');
const failedLifecycles = new Counter('call_lifecycle_failures');
const archiveConsistency = new Rate('call_archive_consistency');
const archiveReadRetries = new Counter('call_archive_read_retries');

export const options = {
  summaryTrendStats: ['min', 'med', 'avg', 'p(90)', 'p(95)', 'p(99)', 'max'],
  scenarios: {
    call_lifecycle: {
      executor: 'constant-vus',
      vus: vuCount,
      duration,
    },
  },
  thresholds: {
    call_lifecycle_success: ['rate>0.99'],
    call_lifecycle_failures: ['count==0'],
    call_archive_consistency: ['rate>0.99'],
    http_req_failed: ['rate<0.01'],
    http_req_duration: ['p(95)<2000'],
  },
};

function jsonBody(response) {
  try {
    return response.json();
  } catch (_error) {
    return null;
  }
}

function passed(response, step, extraChecks = {}) {
  const ok = check(response, {
    [`${step} returns HTTP 200`]: (res) => res.status === 200,
    ...extraChecks,
  });
  if (!ok) {
    failedLifecycles.add(1, { step });
  }
  return ok;
}

export default function () {
  const callSid = `SIM-K6-${runId}-${__VU}-${__ITER}`;
  const params = {
    headers: { 'Content-Type': 'application/json' },
    tags: { scenario: 'call-lifecycle' },
  };

  const incoming = http.post(
    `${api}/incoming`,
    JSON.stringify({
      call_sid: callSid,
      from: '+6590000000',
      to: '+6560000000',
    }),
    params,
  );
  if (!passed(incoming, 'incoming', {
    'incoming response contains the generated session id': (res) => (
      jsonBody(res)?.session_id === callSid
    ),
  })) {
    completedLifecycles.add(false);
    archiveConsistency.add(false);
    sleep(1);
    return;
  }

  const turn = http.post(
    `${api}/${callSid}/turn`,
    JSON.stringify({ transcript: callerTranscript }),
    params,
  );
  const turnOk = passed(turn, 'turn', {
    'turn response contains the matching session id': (res) => (
      jsonBody(res)?.session_id === callSid
    ),
    'turn response contains an assistant reply': (res) => {
      const body = jsonBody(res);
      return typeof body?.assistant_response === 'string' && body.assistant_response.length > 0;
    },
  });

  const hangup = http.post(`${api}/${callSid}/hangup`, null, params);
  const hangupOk = passed(hangup, 'hangup', {
    'hangup response is terminal': (res) => {
      const body = jsonBody(res);
      return body?.session_id === callSid
        && ['completed', 'escalated'].includes(body?.state);
    },
  });

  let archivedSession;
  let archiveResponse;
  for (let attempt = 1; attempt <= archiveReadMaxAttempts; attempt += 1) {
    archiveResponse = http.get(`${api}/${callSid}`, {
      tags: { scenario: 'call-lifecycle', step: 'archive-read' },
    });
    if (archiveResponse.status === 200) {
      archivedSession = jsonBody(archiveResponse);
      break;
    }
    const retryable = archiveResponse.status === 0
      || archiveResponse.status === 408
      || archiveResponse.status === 429
      || archiveResponse.status >= 500;
    if (!retryable) {
      break;
    }
    if (attempt < archiveReadMaxAttempts) {
      archiveReadRetries.add(1);
      sleep(0.2 * attempt);
    }
  }

  const turns = archivedSession?.transcript?.turns;
  const archiveOk = check(archiveResponse, {
    'archived session can be read after hangup': (res) => res.status === 200,
    'archived session id matches the generated call id': () => archivedSession?.id === callSid,
    'archived transcript contains the generated caller turn': () => (
      Array.isArray(turns)
        && turns.some((item) => item.speaker === 'caller' && item.text === callerTranscript)
    ),
    'archived transcript contains an assistant turn': () => (
      Array.isArray(turns) && turns.some((item) => item.speaker === 'assistant')
    ),
  });
  if (!archiveOk) {
    failedLifecycles.add(1, { step: 'archive-read' });
  }

  archiveConsistency.add(archiveOk);
  completedLifecycles.add(turnOk && hangupOk && archiveOk);
  sleep(1);
}
