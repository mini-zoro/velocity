"""Headless tests for aggregation and completeness checks; no child GUI launches."""
import json
import unittest
from measure import aggregate, parse_records, stats, validate_run


class MeasurementTests(unittest.TestCase):
    def test_quantiles_match_rust(self):
        s = stats([4, 1, 2, 3])
        self.assertEqual(s["p50"], 2.5)
        self.assertAlmostEqual(s["p95"], 3.85)
        self.assertIsNone(stats([]))
        self.assertIsNone(stats([float("nan")]))

    def test_phase_and_process_clock_aggregation(self):
        runs = [dict(kind="startup", milestones=[dict(
            phase="first_present_returned", main_ms=n, process_ms=n+5, delta_ms=n)])
            for n in [10, 20, 30, 40]]
        result = aggregate(runs, "startup", "warm")[0]
        self.assertEqual(result["from_main"]["p50"], 25)
        self.assertEqual(result["from_process"]["p50"], 30)
        runs[1]["milestones"][0]["process_ms"] = None
        self.assertIsNone(aggregate(runs, "startup", "warm")[0]["from_process"])
        runs[1]["milestones"][0]["phase"] = "other"
        with self.assertRaises(ValueError):
            aggregate(runs, "startup", "warm")

    def test_jsonl_preserves_unicode_and_escaped_newline(self):
        value = dict(text="你好\n🦀")
        self.assertEqual(parse_records(json.dumps(value) + "\n"), [value])

    def test_never_accepts_partial_or_contaminated_run(self):
        done = dict(kind="complete", complete=True)
        for mode in ["scroll", "typing", "memory"]:
            with self.assertRaises(ValueError):
                validate_run([done], mode)
        with self.assertRaises(ValueError):
            validate_run([done, dict(kind="error", message="GPU lost")], "startup")
        with self.assertRaises(ValueError):
            validate_run([dict(done, external_input_events=1)], "startup")
        validate_run([done], "startup")

    def test_memory_footprint_validation_preserves_legacy_records(self):
        done = dict(kind="complete", complete=True)
        legacy = dict(kind="memory", rss_bytes=100)
        validate_run([legacy, done], "startup")
        validate_run([dict(legacy, phys_footprint_bytes=60), done], "startup")
        with self.assertRaisesRegex(ValueError, "phys_footprint unavailable"):
            validate_run([dict(legacy, phys_footprint_bytes=None,
                               phys_footprint_error="short reply"), done], "startup")


if __name__ == "__main__":
    unittest.main()
