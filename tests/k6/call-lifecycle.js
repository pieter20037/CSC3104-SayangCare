import http from 'k6/http';
import { check, sleep } from 'k6';
import { Counter, Rate } from 'k6/metrics';

const baseUrl = (__ENV.BASE_URL || 'http://127.0.0.1:18080').replace(/\/$/, '');
const api = `${baseUrl}/api/v1/calls`;
const vuCount = Number(__ENV.VUS || 5);
const duration = __ENV.DURATION || '30s';
const runId = __ENV.RUN_ID || `${Date.now()}`;

if (!Number.isInteger(vuCount) || vuCount < 1) {
  throw new Error('VUS must be a positive integer.');
}

const completedLifecycles = new Rate('call_lifecycle_success');
const failedLifecycles = new Counter('call_lifecycle_failures');

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
    http_req_failed: ['rate<0.01'],
    http_req_duration: ['p(95)<2000'],
  },
};

function passed(response, step) {
  const ok = check(response, {
    [`${step} returns HTTP 200`]: (res) => res.status === 200,
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
  if (!passed(incoming, 'incoming')) {
    completedLifecycles.add(false);
    sleep(1);
    return;
  }

  const turn = http.post(
    `${api}/${callSid}/turn`,
    JSON.stringify({ transcript: 'I have been feeling overwhelmed by a busy day.' }),
    params,
  );
  const turnOk = passed(turn, 'turn');

  const hangup = http.post(`${api}/${callSid}/hangup`, null, params);
  const hangupOk = passed(hangup, 'hangup');

  completedLifecycles.add(turnOk && hangupOk);
  sleep(1);
}
