"""Offline validation of real-provider evidence gates; no API credits used."""
import importlib.util
from pathlib import Path
import unittest
import tempfile

spec = importlib.util.spec_from_file_location('perf_benchmark', Path(__file__).with_name('perf-benchmark.py'))
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)

def sample(stage, outcome, count=1, path='json'):
    return f'sayangcare_caller_turn_stage_duration_seconds_count{{outcome="{outcome}",path="{path}",stage="{stage}"}} {count}\n'

class RealAiEvidenceTests(unittest.TestCase):
    def test_missing_key_blocks_live_request(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, 'Real AI test blocked'):
                benchmark.real_ai_settings({}, Path(directory) / '.env')

    def test_exact_env_key_and_process_precedence(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / '.env'
            path.write_text('SAYANGCARE__AI__API_KEY="local-test-value"\nSAYANGCARE__AI__MODEL=test-model\n')
            settings = benchmark.real_ai_settings({'SAYANGCARE__AI__API_KEY': 'process-value'}, path)
            self.assertEqual(settings['SAYANGCARE__AI__API_KEY'], 'process-value')
            self.assertEqual(settings['SAYANGCARE__AI__MODEL'], 'test-model')

    def test_live_key_cannot_be_sent_to_unapproved_endpoint(self):
        for endpoint in ('http://api.groq.com/test', 'https://example.test', 'https://user:pass@api.groq.com/test'):
            with self.assertRaises(ValueError):
                benchmark.real_ai_settings({'SAYANGCARE__AI__API_KEY': 'local-test-value',
                                            'SAYANGCARE__AI__BASE_URL': endpoint}, Path('absent-test-env'))

    def test_all_errors_cannot_pass_as_real_ai_success(self):
        with self.assertRaises(AssertionError):
            benchmark.validate_real_ai(sample('sentiment', 'error') + sample('reply', 'timeout'), [{'audio_fallback': True}])

    def test_successful_reply_requires_successful_sentiment(self):
        with self.assertRaises(AssertionError):
            benchmark.validate_real_ai(sample('sentiment', 'timeout') + sample('reply', 'ok'), [{'audio_fallback': False}])

    def test_partial_success_cannot_pass(self):
        metrics = sample('sentiment', 'ok') + sample('reply', 'ok') + sample('reply', 'timeout', 2)
        with self.assertRaises(AssertionError):
            benchmark.validate_real_ai(metrics, [{'audio_fallback': False}, {'audio_fallback': True}])

    def test_all_samples_must_have_both_operations(self):
        metrics = sample('sentiment', 'ok', 3) + sample('reply', 'ok', 3)
        result = benchmark.validate_real_ai(metrics, [{'audio_fallback': False}] * 3)
        self.assertEqual(result['reply']['ok'], 3)
        with self.assertRaises(AssertionError):
            benchmark.validate_real_ai(metrics, [{'audio_fallback': False}] * 4)

    def test_twilio_or_zero_samples_cannot_satisfy_json_evidence(self):
        for metrics in (sample('sentiment', 'ok', path='twilio') + sample('reply', 'ok', path='twilio'), sample('sentiment', 'ok', 0) + sample('reply', 'ok', 0)):
            with self.assertRaises(AssertionError):
                benchmark.validate_real_ai(metrics, [{'audio_fallback': False}])

if __name__ == '__main__':
    unittest.main()
