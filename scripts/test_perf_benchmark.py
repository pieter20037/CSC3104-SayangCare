"""Offline validation of real-provider evidence gates; no API credits used."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('perf_benchmark', Path(__file__).with_name('perf-benchmark.py'))
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)

def sample(stage, outcome, count=1, path='json'):
    return f'sayangcare_caller_turn_stage_duration_seconds_count{{outcome="{outcome}",path="{path}",stage="{stage}"}} {count}\n'

class RealAiEvidenceTests(unittest.TestCase):
    def test_all_errors_cannot_pass_as_real_ai_success(self):
        with self.assertRaises(AssertionError):
            benchmark.validate_real_ai(sample('sentiment', 'error') + sample('reply', 'timeout'), [{'audio_fallback': True}])

    def test_successful_reply_requires_successful_sentiment(self):
        with self.assertRaises(AssertionError):
            benchmark.validate_real_ai(sample('sentiment', 'timeout') + sample('reply', 'ok'), [{'audio_fallback': False}])

    def test_valid_success_keeps_failures_visible(self):
        metrics = sample('sentiment', 'ok') + sample('reply', 'ok') + sample('reply', 'timeout', 2)
        result = benchmark.validate_real_ai(metrics, [{'audio_fallback': False}, {'audio_fallback': True}])
        self.assertEqual(result['reply'], {'ok': 1, 'timeout': 2})

    def test_twilio_or_zero_samples_cannot_satisfy_json_evidence(self):
        for metrics in (sample('sentiment', 'ok', path='twilio') + sample('reply', 'ok', path='twilio'), sample('sentiment', 'ok', 0) + sample('reply', 'ok', 0)):
            with self.assertRaises(AssertionError):
                benchmark.validate_real_ai(metrics, [{'audio_fallback': False}])

if __name__ == '__main__':
    unittest.main()
